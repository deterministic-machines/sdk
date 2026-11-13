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
  | { stdin: { data?: string; close?: boolean } }
  | { signal: number }
  | { write: { path: string; data: string; mode?: number; last: boolean } }
  | { read: { path: string } };
type Answer = { id?: number } & (
  | { started: { pid: number; argv: string[] } }
  | { stdout: string } | { stderr: string } | { exited: { code: number } }
  | { written: boolean } | { data: string } | { end: boolean } | { error: string }
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

/** The only component that knows byte offsets and protocol lines. */
export class SandboxSession {
  static readonly MAX_LINE = 65536;
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
        for (const action of this.#pieces(op.id, "write", { path, mode: options.mode }, data)) {
          op.check();
          await this.#post(op.id, action, options);
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
        for (const action of this.#pieces(operation.id, "stdin", {}, data)) {
          operation.check();
          await this.#post(operation.id, action, options);
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
    const state = await this.#calls.whileLive(() => this.#calls.connection.json<{ length: number }>(
      "POST", `${this.#calls.path}/input`, { ...options,
        body: { offset: this.#offset ?? 0, chunk: encode(chunk), end } },
    ));
    if (!Number.isSafeInteger(state.length) || state.length < 0) {
      const error = new DvmError("input length exceeds JavaScript's safe integers");
      this.#fail(error);
      throw error;
    }
    this.#offset = state.length;
  }

  #line(id: number, action: RequestAction): Uint8Array {
    return new TextEncoder().encode(JSON.stringify({ id, ...action }) + "\n");
  }

  async #post(id: number, action: RequestAction, options: CallOptions): Promise<void> {
    const line = this.#line(id, action);
    if (line.length > SandboxSession.MAX_LINE) throw new DvmError("request line exceeds 65536 bytes");
    await this.#append(line, options);
  }

  *#pieces(id: number, kind: "stdin" | "write", fields: { path?: string; mode?: number }, data: Uint8Array): Generator<RequestAction> {
    const envelope = kind === "write"
      ? { write: { path: fields.path!, mode: fields.mode, data: "", last: false } }
      : { stdin: { data: "" } };
    const size = Math.floor((SandboxSession.MAX_LINE - this.#line(id, envelope).length) / 4) * 3;
    if (size < 1) throw new DvmError("request line exceeds 65536 bytes");
    for (let at = 0; at < Math.max(1, data.length); at += size) {
      const piece = data.subarray(at, at + size);
      yield kind === "write"
        ? { write: { path: fields.path!, mode: fields.mode, data: encode(piece), last: at + piece.length === data.length } }
        : { stdin: { data: encode(piece) } };
    }
  }

  async #readAnswers(): Promise<void> {
    let offset = 0, stderrOffset = 0;
    let pending: Uint8Array = new Uint8Array();
    try {
      for (;;) {
        const output = await this.#calls.connection.json<{ stdout: string; stderr: string; more: boolean }>(
          "GET", `/executions/${this.#calls.id}/output`, {
            query: { offset, stderr_offset: stderrOffset }, signal: this.#reading.signal,
          });
        const chunk = decode(output.stdout);
        offset += chunk.length;
        stderrOffset += decode(output.stderr).length;
        pending = join([pending, chunk]);
        let end: number;
        while ((end = pending.indexOf(10)) >= 0) {
          this.#dispatch(JSON.parse(new TextDecoder().decode(pending.subarray(0, end))) as Answer);
          pending = pending.slice(end + 1);
        }
        if (!output.more) {
          const code = await this.#wait();
          throw new SandboxEndedError(this.#calls.id, new RunState({ kind: "exited", code }));
        }
      }
    } catch (error) { this.#fail(error); }
  }

  #dispatch(answer: Answer): void {
    if ("error" in answer && answer.id === undefined) {
      this.#failOperations(new DvmError(answer.error));
      return;
    }
    const operation = this.#operations.get(answer.id!);
    if (!operation) return;
    if ("error" in answer) operation.fail(new DvmError(answer.error));
    else if ("started" in answer) operation.started = true;
    else if ("stdout" in answer) operation.stdout.push(decode(answer.stdout));
    else if ("stderr" in answer) operation.stderr.push(decode(answer.stderr));
    else if ("data" in answer) operation.data.push(decode(answer.data));
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

function encode(bytes: Uint8Array): string {
  return btoa(Array.from(bytes, (byte) => String.fromCharCode(byte)).join(""));
}
function decode(encoded: string): Uint8Array {
  return Uint8Array.from(atob(encoded), (byte) => byte.charCodeAt(0));
}
function join(parts: readonly Uint8Array[]): Uint8Array {
  const bytes = new Uint8Array(parts.reduce((size, part) => size + part.length, 0));
  let at = 0;
  for (const part of parts) { bytes.set(part, at); at += part.length; }
  return bytes;
}
