/** Commands in a sandbox: run one and get its output, or start one in the
 * background and hold a handle to it. */

import type { ApiCommand, ApiCommandKind, ApiOutputEvent } from "./api-json.ts";
import { type CallOptions, NetworkError } from "./connection.ts";
import { CommandRefusedError, DvmError } from "./errors.ts";
import type { SandboxCalls } from "./sandbox-calls.ts";

/** A script, which the sandbox's shell runs (`/bin/bash -lc` if the image
 * has bash, else `/bin/sh -c`), or an argv, which runs as it is. */
export type CommandLine = string | readonly string[];

/** Called with each piece of a stream's output, as text, as it's written. */
export type OutputCallback = (text: string) => void;

const SIGKILL = 9;

/** A command that exited: its output, and its exit code. A signal's kill is
 * code 128 and the signal, as a shell says, with the signal given. */
export interface CommandResult {
  readonly number: number;
  readonly stdout: string;
  readonly stderr: string;
  readonly exitCode: number;
  readonly signal: number | null;
}

/** A command as its execution recorded it: `state` is `starting`,
 * `running`, `exited` (with `exitCode`), `refused` (with `reason`), or
 * `unfinished` as its sandbox ended first. Times are milliseconds since the
 * Unix epoch. */
export interface CommandInfo {
  readonly number: number;
  readonly command: CommandLine;
  readonly argv: string[] | null;
  readonly state: ApiCommandKind;
  readonly exitCode: number | null;
  readonly signal: number | null;
  readonly reason: string | null;
  readonly startedAt: number | null;
  readonly endedAt: number | null;
}

/** How a command starts. */
export interface StartOptions extends CallOptions {
  cwd?: string;
  env?: Record<string, string>;
  /** `true` keeps its stdin open for `sendStdin`; text or bytes are sent to
   * it, and it's closed after. Else it reads `/dev/null`. */
  stdin?: boolean | string | Uint8Array;
}

/** How a command is waited for. */
export interface WaitOptions extends CallOptions {
  /** Once it passes, the command is killed (SIGKILL), and its result says
   * so. */
  timeoutMs?: number;
  onStdout?: OutputCallback;
  onStderr?: OutputCallback;
}

export interface RunOptions extends StartOptions, WaitOptions {
  background?: false;
}

export interface BackgroundOptions extends StartOptions {
  background: true;
}

/** A sandbox's commands: `sandbox.commands`. */
export class Commands {
  readonly #calls: SandboxCalls;

  constructor(calls: SandboxCalls) {
    this.#calls = calls;
  }

  /** Runs `command` to its exit: its result. A `signal` that aborts kills
   * it. With `background: true`, a handle to it at once instead, whose
   * `wait` takes `timeoutMs` and the callbacks. */
  run(command: CommandLine, options: BackgroundOptions): Promise<CommandHandle>;
  run(command: CommandLine, options?: RunOptions): Promise<CommandResult>;
  async run(
    command: CommandLine,
    options: RunOptions | BackgroundOptions = {},
  ): Promise<CommandResult | CommandHandle> {
    const waiting: WaitOptions = options;
    if (options.background && (waiting.timeoutMs !== undefined || waiting.onStdout || waiting.onStderr)) {
      throw new TypeError("a background command takes timeoutMs and callbacks in wait()");
    }
    const started: Record<string, unknown> = {
      command: typeof command === "string" ? command : [...command],
    };
    if (options.cwd !== undefined) started.cwd = options.cwd;
    if (options.env && Object.keys(options.env).length > 0) started.env = options.env;
    if (options.stdin !== undefined && options.stdin !== false) started.stdin = true;
    const calls = this.#calls;
    const { signal } = options;
    const { number } = await calls.whileLive(() =>
      calls.connection.json<{ number: number }>("POST", `${calls.path}/commands`, {
        body: started,
        signal,
      }),
    );
    const handle = new CommandHandle(calls, number);
    if (typeof options.stdin === "string" || options.stdin instanceof Uint8Array) {
      await handle.sendStdin(options.stdin, { close: true, signal });
    }
    if (options.background) return handle;
    return waitFor(calls, handle, waiting, { killOnAbort: true });
  }
}

/** A command started in the background. */
export class CommandHandle {
  readonly number: number;
  readonly #calls: SandboxCalls;
  readonly #path: string;

  constructor(calls: SandboxCalls, number: number) {
    this.number = number;
    this.#calls = calls;
    this.#path = `${calls.path}/commands/${number}`;
  }

  /** Its result once it exits, with all it wrote. The callbacks get its
   * output from its start, as it's written. A `signal` that aborts stops
   * the waiting, not the command. */
  wait(options: WaitOptions = {}): Promise<CommandResult> {
    return waitFor(this.#calls, this, options, { killOnAbort: false });
  }

  /** Sends `data` to its stdin, which it was started with open. */
  async sendStdin(data: string | Uint8Array, options: CallOptions & { close?: boolean } = {}): Promise<void> {
    const content = typeof data === "string" ? new TextEncoder().encode(data) : data;
    const query = options.close ? { close: true } : undefined;
    const calls = this.#calls;
    await calls.whileLive(() =>
      calls.connection.bytes("POST", `${this.#path}/stdin`, { query, content, signal: options.signal }),
    );
  }

  /** Closes its stdin: it reads no more than it was sent. */
  async closeStdin(options: CallOptions = {}): Promise<void> {
    await this.sendStdin(new Uint8Array(), { ...options, close: true });
  }

  /** Sends `signal`, SIGKILL unless given, to its process (not what that
   * started). */
  async kill(signal: number = SIGKILL, options: CallOptions = {}): Promise<void> {
    const calls = this.#calls;
    await calls.whileLive(() =>
      calls.connection.json("POST", `${this.#path}/signal`, { body: { signal }, signal: options.signal }),
    );
  }

  /** The command as it is now. */
  async info(options: CallOptions = {}): Promise<CommandInfo> {
    return commandInfoOf(await this.#calls.connection.json<ApiCommand>("GET", this.#path, options));
  }
}

export function commandInfoOf(command: ApiCommand): CommandInfo {
  const { state } = command;
  return {
    number: command.number,
    command: command.command,
    argv: command.argv,
    state: state.kind,
    exitCode: state.code ?? null,
    signal: state.signal ?? null,
    reason: state.reason ?? null,
    startedAt: command.started_at,
    endedAt: command.ended_at,
  };
}

/** The command's result once it exits; past `timeoutMs`, killed first. */
async function waitFor(
  calls: SandboxCalls,
  handle: CommandHandle,
  options: WaitOptions,
  { killOnAbort }: { killOnAbort: boolean },
): Promise<CommandResult> {
  const followed = new FollowedOutput(calls, handle.number, options);
  const { signal, timeoutMs } = options;
  const timeout = new AbortController();
  const timer = timeoutMs === undefined ? undefined : setTimeout(() => timeout.abort(), timeoutMs);
  try {
    await followed.follow(signal ? AbortSignal.any([signal, timeout.signal]) : timeout.signal);
  } catch (error) {
    if (signal?.aborted) {
      if (killOnAbort) await handle.kill().catch(() => undefined);
      throw error;
    }
    if (!timeout.signal.aborted) throw error;
    await handle.kill(SIGKILL, { signal });
    await followed.follow(signal);
  } finally {
    clearTimeout(timer);
  }
  return followed.result();
}

/** A command's output, followed from its start to its end over server-sent
 * events; a stream that drops is followed again from where it got to. */
class FollowedOutput {
  // Streams in a row that end before the command, at once and with nothing
  // new: one that went on a while was cut, by the network or a proxy.
  static readonly MOST_FRUITLESS_FOLLOWS = 3;
  static readonly CUT_AFTER_MS = 1000;

  readonly #calls: SandboxCalls;
  readonly #path: string;
  readonly #stdout: StreamOutput;
  readonly #stderr: StreamOutput;
  #command: CommandInfo | null = null;

  constructor(calls: SandboxCalls, number: number, options: WaitOptions) {
    this.#calls = calls;
    this.#path = `${calls.path}/commands/${number}/output`;
    this.#stdout = new StreamOutput(options.onStdout);
    this.#stderr = new StreamOutput(options.onStderr);
  }

  /** Returns once the command ended. */
  async follow(signal?: AbortSignal): Promise<void> {
    let fruitless = 0;
    while (this.#command === null || isActive(this.#command)) {
      const written = this.#written();
      const started = performance.now();
      let dropped: unknown;
      try {
        await this.#followFromOffsets(signal);
      } catch (error) {
        if (!(error instanceof NetworkError) || signal?.aborted) throw error;
        dropped = error;
      }
      const cut = performance.now() - started > FollowedOutput.CUT_AFTER_MS;
      fruitless = this.#written() > written || cut ? 0 : fruitless + 1;
      if (fruitless > FollowedOutput.MOST_FRUITLESS_FOLLOWS) {
        throw new DvmError(`${this.#path}'s stream ended before its command`, { cause: dropped });
      }
    }
  }

  async result(): Promise<CommandResult> {
    const command = this.#command;
    if (command === null || isActive(command)) {
      throw new DvmError(`${this.#path} wasn't followed to its end`);
    }
    if (command.state === "refused") {
      throw new CommandRefusedError(command.number, command.reason ?? "");
    }
    if (command.state === "unfinished") {
      const ended = await this.#calls.endedError();
      throw ended ?? new DvmError(`command ${command.number} is unfinished`);
    }
    return {
      number: command.number,
      stdout: this.#stdout.finish(),
      stderr: this.#stderr.finish(),
      exitCode: command.exitCode ?? 0,
      signal: command.signal,
    };
  }

  async #followFromOffsets(signal?: AbortSignal): Promise<void> {
    const query = {
      follow: true,
      stdout_offset: this.#stdout.length,
      stderr_offset: this.#stderr.length,
    };
    for await (const event of this.#calls.connection.events(this.#path, { query, signal })) {
      this.#take(event as ApiOutputEvent);
    }
  }

  #written(): number {
    return this.#stdout.length + this.#stderr.length;
  }

  #take(event: ApiOutputEvent): void {
    if (event.kind === "command") {
      this.#command = commandInfoOf(event.command);
      return;
    }
    const stream = event.stream === "stdout" ? this.#stdout : this.#stderr;
    stream.take(event.offset, bytesOfBase64(event.chunk));
  }
}

/** One stream's bytes so far, and its text to a callback as it comes: a
 * character split across chunks waits for its end. */
class StreamOutput {
  readonly #chunks: Uint8Array[] = [];
  readonly #callback: OutputCallback | undefined;
  readonly #decoder = new TextDecoder();
  #length = 0;

  constructor(callback?: OutputCallback) {
    this.#callback = callback;
  }

  get length(): number {
    return this.#length;
  }

  take(offset: number, chunk: Uint8Array): void {
    if (offset > this.#length) {
      throw new DvmError(`output from byte ${offset}, after only ${this.#length}`);
    }
    const fresh = chunk.subarray(this.#length - offset);
    if (fresh.length === 0) return;
    this.#chunks.push(fresh);
    this.#length += fresh.length;
    if (this.#callback) {
      const text = this.#decoder.decode(fresh, { stream: true });
      if (text) this.#callback(text);
    }
  }

  finish(): string {
    if (this.#callback) {
      const text = this.#decoder.decode();
      if (text) this.#callback(text);
    }
    const all = new Uint8Array(this.#length);
    let at = 0;
    for (const chunk of this.#chunks) {
      all.set(chunk, at);
      at += chunk.length;
    }
    return new TextDecoder().decode(all);
  }
}

function isActive(command: CommandInfo): boolean {
  return command.state === "starting" || command.state === "running";
}

function bytesOfBase64(text: string): Uint8Array {
  return Uint8Array.from(atob(text), (character) => character.charCodeAt(0));
}
