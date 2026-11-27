import { FrameDecoder, MAX_HEADER, decodeOutput, encodeFrame, join } from "./frame.ts";
import type { CallOptions } from "./connection.ts";
import { DvmError, SandboxEndedError } from "./errors.ts";
import { RunState } from "./run-state.ts";
import type { SandboxCalls } from "./sandbox-calls.ts";

/** A command's exit code (128 + killing signal) and separate byte streams. */
export interface CommandResult {
  readonly exitCode: number;
  readonly stdout: Uint8Array;
  readonly stderr: Uint8Array;
}

export interface CommandOptions extends CallOptions {
  env?: Record<string, string>;
  cwd?: string;
  stdin?: Uint8Array;
  /** As `docker exec --user`: "name", "uid", "name:group". Omitted, the sandbox server's user. */
  user?: string;
}

export interface WriteFileOptions extends CallOptions {
  mode?: number;
}

// This package does not carry dvm-api's TypeScript export.
type RequestAction =
  | { run: ({ argv: string[] } | { script: string }) & { env?: Record<string, string>; cwd?: string; stdin: boolean; user?: string } }
  | { stdin: { close?: boolean } }
  | { signal: number }
  | { write: { path: string; mode?: number; last: boolean } }
  | { read: { path: string } };
type Answer = { id?: number } & (
  | { started: { pid: number; argv: string[] } }
  | { stdout: boolean } | { stderr: boolean } | { exited: { code: number } }
  | { written: boolean } | { data: boolean } | { end: boolean } | { error: string }
);

class Operation {
  readonly id: number;
  started = false;
  done = false;
  error: unknown;
  result?: CommandResult;
  readonly stdout: Uint8Array[] = [];
  readonly stderr: Uint8Array[] = [];
  readonly data: Uint8Array[] = [];
  readonly #listeners = new Set<() => void>();

  constructor(id: number) { this.id = id; }

  check(): void { if (this.error !== undefined) throw this.error; }

  notify(): void { for (const wake of [...this.#listeners]) wake(); }

  fail(error: unknown): void { this.error = error; this.notify(); }

  async until(ready: () => boolean, signal?: AbortSignal): Promise<void> {
    for (;;) {
      signal?.throwIfAborted();
      this.check();
      if (ready()) return;
      await new Promise<void>((resolve, reject) => {
        const cleanup = () => {
          this.#listeners.delete(wake);
          signal?.removeEventListener("abort", abort);
        };
        const wake = () => { cleanup(); resolve(); };
        const abort = () => { cleanup(); reject(signal!.reason); };
        this.#listeners.add(wake);
        signal?.addEventListener("abort", abort, { once: true });
      });
    }
  }
}

/** A started command. Omitted stdin stays open for this handle. */
export class Command {
  readonly #session: SandboxSession;
  readonly #operation: Operation;
  #stdinClosed = false;

  constructor(session: SandboxSession, operation: Operation) {
    this.#session = session;
    this.#operation = operation;
  }

  async wait(options: CallOptions = {}): Promise<CommandResult> {
    await this.#operation.until(() => this.#operation.done, options.signal);
    return this.#operation.result!;
  }

  async signal(number: number, options: CallOptions = {}): Promise<void> {
    if (!Number.isInteger(number) || number < 1 || number > 64) throw new DvmError("signal must be between 1 and 64");
    await this.#session.send(this.#operation, { signal: number }, options);
  }

  async writeStdin(data: Uint8Array, options: CallOptions = {}): Promise<void> {
    await this.#session.stdin(this.#operation, data, false, () => this.#stdinClosed, options);
  }

  async closeStdin(options: CallOptions = {}): Promise<void> {
    await this.#session.stdin(this.#operation, new Uint8Array(), true, () => this.#stdinClosed, options,
      () => { this.#stdinClosed = true; });
  }
}

/** The only component that knows byte offsets and protocol frames. */
export class SandboxSession {
  static readonly PIECE_SIZE = 1024 * 1024;
  readonly #calls: SandboxCalls;
  readonly #wait: (options?: CallOptions) => Promise<number>;
  readonly #operations = new Map<number, Operation>();
  readonly #reading = new AbortController();
  #offset: number | undefined;
  #ended = false;
  #fatal: unknown;
  #writer = Promise.resolve();
  #reader?: Promise<void>;

  constructor(calls: SandboxCalls, wait: (options?: CallOptions) => Promise<number>, offset?: number) {
    this.#calls = calls;
    this.#wait = wait;
    this.#offset = offset;
  }

  async start(command: string[] | string, options: CommandOptions): Promise<Command> {
    const program = typeof command === "string" ? { script: command } : { argv: command };
    const operation = await this.#begin({ run: { ...program, env: options.env, cwd: options.cwd, stdin: true, user: options.user } }, options);
    await operation.until(() => operation.started, options.signal);
    const handle = new Command(this, operation);
    if (options.stdin !== undefined) {
      if (options.stdin.length) await handle.writeStdin(options.stdin, options);
      await handle.closeStdin(options);
    }
    return handle;
  }

  async writeFile(path: string, data: Uint8Array, options: WriteFileOptions): Promise<void> {
    const operation = await this.#writing(async () => {
      await this.#ready(options);
      const op = this.#register();
      try {
        for (const [action, payload] of this.#pieces("write", { path, mode: options.mode }, data)) {
          op.check();
          await this.#post(op.id, action, options, payload);
        }
      } catch (error) { this.#operations.delete(op.id); throw error; }
      return op;
    });
    await operation.until(() => operation.done, options.signal);
  }

  async readFile(path: string, options: CallOptions): Promise<Uint8Array> {
    const operation = await this.#begin({ read: { path } }, options);
    await operation.until(() => operation.done, options.signal);
    return join(operation.data);
  }

  async end(options: CallOptions): Promise<number> {
    await this.#writing(async () => {
      if (!this.#ended) {
        await this.#ready(options, true);
        await this.#append(new Uint8Array(), options, true);
        this.#ended = true;
      }
    });
    return this.#wait(options);
  }

  async close(): Promise<void> {
    const error = await this.#calls.endedError() ?? new DvmError("sandbox session closed");
    this.#fail(error);
    this.#reading.abort();
    await this.#reader;
  }

  async send(operation: Operation, action: RequestAction, options: CallOptions): Promise<void> {
    await this.#writing(async () => {
      await this.#ready(options);
      operation.check();
      await this.#post(operation.id, action, options);
    });
  }

  async stdin(operation: Operation, data: Uint8Array, close: boolean, closed: () => boolean,
    options: CallOptions, didClose?: () => void): Promise<void> {
    await this.#writing(async () => {
      if (closed()) {
        if (close) return;
        throw new DvmError("command stdin is closed");
      }
      await this.#ready(options);
      operation.check();
      if (close) {
        await this.#post(operation.id, { stdin: { close: true } }, options);
        didClose!();
      } else {
        for (const [action, payload] of this.#pieces("stdin", {}, data)) {
          operation.check();
          await this.#post(operation.id, action, options, payload);
        }
      }
    });
  }

  async #begin(action: RequestAction, options: CallOptions): Promise<Operation> {
    return this.#writing(async () => {
      await this.#ready(options);
      const operation = this.#register();
      try { await this.#post(operation.id, action, options); }
      catch (error) { this.#operations.delete(operation.id); throw error; }
      return operation;
    });
  }

  #register(): Operation {
    if (this.#fatal !== undefined) throw this.#fatal;
    const operation = new Operation(this.#offset!);
    this.#operations.set(operation.id, operation);
    return operation;
  }

  async #writing<T>(write: () => Promise<T>): Promise<T> {
    const previous = this.#writer;
    let release!: () => void;
    this.#writer = new Promise<void>((resolve) => { release = resolve; });
    await previous;
    try { return await write(); } finally { release(); }
  }

  async #ready(options: CallOptions, ending = false): Promise<void> {
    options.signal?.throwIfAborted();
    if (!ending && this.#fatal !== undefined) throw this.#fatal;
    if (this.#ended) throw new DvmError("sandbox input has ended");
    if (this.#offset === undefined) await this.#append(new Uint8Array(), options);
    this.#reader ??= this.#readAnswers();
  }

  async #append(chunk: Uint8Array, options: CallOptions, end = false): Promise<void> {
    const state = await this.#calls.whileLive(() => this.#calls.connection.input<{ length: number }>(
      `${this.#calls.path}/input`, chunk, { ...options,
        query: { offset: this.#offset ?? 0, end } },
    ));
    if (!Number.isSafeInteger(state.length) || state.length < 0) {
      const error = new DvmError("input length exceeds JavaScript's safe integers");
      this.#fail(error);
      throw error;
    }
    this.#offset = state.length;
  }

  async #post(id: number, action: RequestAction, options: CallOptions, payload: Uint8Array = new Uint8Array()): Promise<void> {
    const frame = encodeFrame({ id, ...action }, payload);
    if (frame.length - payload.length - 1 > MAX_HEADER) throw new DvmError("request header exceeds 65536 bytes");
    for (let at = 0; at < frame.length; at += 65536) await this.#append(frame.subarray(at, at + 65536), options);
  }

  *#pieces(kind: "stdin" | "write", fields: { path?: string; mode?: number }, data: Uint8Array): Generator<[RequestAction, Uint8Array]> {
    for (let at = 0; at < Math.max(1, data.length); at += SandboxSession.PIECE_SIZE) {
      const piece = data.subarray(at, at + SandboxSession.PIECE_SIZE);
      const action = kind === "write"
        ? { write: { path: fields.path!, mode: fields.mode, last: at + piece.length === data.length } }
        : { stdin: {} };
      yield [action, piece];
    }
  }

  async #readAnswers(): Promise<void> {
    let offset = 0, stderrOffset = 0;
    const decoder = new FrameDecoder();
    try {
      for (;;) {
        const output = decodeOutput(await this.#calls.connection.bytes(`${this.#calls.path}/output`, {
          query: { offset, stderr_offset: stderrOffset }, signal: this.#reading.signal,
        }));
        offset += output.stdout.length;
        stderrOffset += output.stderr.length;
        decoder.push(output.stdout);
        for (;;) {
          const frame = decoder.nextFrame<Answer>();
          if (!frame) break;
          this.#dispatch(frame.header, frame.payload);
        }
        if (!output.more && !output.cut) {
          const code = await this.#wait();
          throw new SandboxEndedError(this.#calls.id, new RunState({ kind: "exited", code }));
        }
      }
    } catch (error) { this.#fail(error); }
  }

  #dispatch(answer: Answer, payload: Uint8Array): void {
    if ("error" in answer && answer.id === undefined) {
      this.#failOperations(new DvmError(answer.error));
      return;
    }
    const operation = this.#operations.get(answer.id!);
    if (!operation) return;
    if ("error" in answer) operation.fail(new DvmError(answer.error));
    else if ("started" in answer) operation.started = true;
    else if ("stdout" in answer) operation.stdout.push(payload);
    else if ("stderr" in answer) operation.stderr.push(payload);
    else if ("data" in answer) operation.data.push(payload);
    else if ("exited" in answer) {
      operation.result = { exitCode: answer.exited.code, stdout: join(operation.stdout), stderr: join(operation.stderr) };
      operation.done = true;
    } else if ("written" in answer || "end" in answer) operation.done = true;
    operation.notify();
    if (operation.done || operation.error !== undefined) this.#operations.delete(operation.id);
  }

  #failOperations(error: unknown): void {
    for (const operation of this.#operations.values()) operation.fail(error);
    this.#operations.clear();
  }

  #fail(error: unknown): void {
    this.#fatal ??= error;
    this.#failOperations(this.#fatal);
  }
}

