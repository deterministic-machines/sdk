import type { ApiRun, ApiSandbox } from "./api-json.ts";
import { type CallOptions, type ClientOptions, Connection } from "./connection.ts";
import { ConflictError, DvmError } from "./errors.ts";
import { type Output, outputOf } from "./execution.ts";
import { RunState } from "./run-state.ts";
import { endedErrorOf, SandboxCalls } from "./sandbox-calls.ts";
import { Command, type CommandOptions, type CommandResult, SandboxSession, type WriteFileOptions } from "./sandbox-session.ts";

/** What a sandbox is made of; left out: 1 vCPU, 1024 MiB, seed 0, no
 * variables beyond the image's, and named after the image. */
export interface SandboxOptions extends ClientOptions, CallOptions {
  command?: string[];
  entrypoint?: string;
  cpus?: number;
  memoryMib?: number;
  seed?: number;
  env?: Record<string, string>;
  name?: string;
}

export class Sandbox implements AsyncDisposable {
  /** Its execution's id too. */
  readonly id: number;
  readonly name: string;
  readonly #calls: SandboxCalls;
  readonly #takesCommands: boolean;
  readonly #inputOffset: number | undefined;
  #session?: SandboxSession;

  /** A handle, over `connection`, to the sandbox the API answered with:
   * `Sandbox.create` and `Sandbox.connect` make one. */
  constructor(connection: Connection, sandbox: { id: number; spec: { name: string; command?: string[]; entrypoint?: string | null } }, inputOffset?: number) {
    this.id = sandbox.id;
    this.name = sandbox.spec.name;
    this.#calls = new SandboxCalls(connection, sandbox.id);
    this.#takesCommands = !sandbox.spec.command?.length && sandbox.spec.entrypoint == null;
    this.#inputOffset = inputOffset;
  }

  /** A sandbox of `image` (as `docker run` takes it), once it's running.
   * Without command or entrypoint it takes commands, replacing the image's
   * own program. A `signal` that aborts while it starts stops it. */
  static async create(image: string, options: SandboxOptions = {}): Promise<Sandbox> {
    const draft: Record<string, unknown> = { image };
    if (options.command !== undefined) draft.command = options.command;
    if (options.entrypoint !== undefined) draft.entrypoint = options.entrypoint;
    if (options.cpus !== undefined) draft.cpus = options.cpus;
    if (options.memoryMib !== undefined) draft.memory_mib = options.memoryMib;
    if (options.seed !== undefined) draft.seed = options.seed;
    if (options.name !== undefined) draft.name = options.name;
    const env = Object.entries(options.env ?? {});
    if (env.length > 0) draft.environment = env.map(([name, value]) => ({ name, value }));
    const connection = await Connection.open(options);
    const { signal } = options;
    const created = await connection.json<ApiSandbox>("POST", "/sandboxes", { body: draft, signal });
    const sandbox = new Sandbox(connection, created, 0);
    try {
      await sandbox.#waitUntilRunning(created.run, signal);
    } catch (error) {
      if (signal?.aborted) {
        await connection.json("POST", `${sandbox.#calls.path}/stop`).catch(() => undefined);
      }
      throw error;
    }
    return sandbox;
  }

  /** A handle to a sandbox made elsewhere. */
  static async connect(sandboxId: number, options: ClientOptions & CallOptions = {}): Promise<Sandbox> {
    const connection = await Connection.open(options);
    const sandbox = await connection.json<ApiSandbox>("GET", `/sandboxes/${sandboxId}`, options);
    return new Sandbox(connection, sandbox);
  }

  /** Where it is: `queued`, `running`, `paused`, `exited`, `stopped`, `failed`. */
  state(options: CallOptions = {}): Promise<RunState> {
    return this.#calls.state(options);
  }

  async pause(options: CallOptions = {}): Promise<void> {
    await this.#act("pause", options);
  }

  async resume(options: CallOptions = {}): Promise<void> {
    await this.#act("resume", options);
  }

  /** Stops an active run. Its cut recording cannot replay. */
  async stop(options: CallOptions = {}): Promise<void> {
    const calls = this.#calls;
    let run: ApiRun;
    try {
      run = await calls.connection.json<ApiRun>("POST", `${calls.path}/stop`, options);
    } catch (refused) {
      if (!(refused instanceof ConflictError)) throw refused;
      run = (await calls.connection.json<ApiSandbox>("GET", calls.path, options)).run;
      if (new RunState(run.state).isActive) throw refused;
    }
  }

  /** The program's exit code; stopped and failed runs throw their error. */
  async wait(options: CallOptions = {}): Promise<number> {
    let run = (await this.#calls.connection.json<ApiSandbox>("GET", this.#calls.path, options)).run;
    for (;;) {
      const state = new RunState(run.state);
      if (run.state.kind === "exited") return run.state.code!;
      if (!state.isActive) throw endedErrorOf(this.id, state);
      run = (await this.#newerThan(run, options.signal)).run;
    }
  }

  async run(command: string[] | string, options: CommandOptions = {}): Promise<CommandResult> {
    const handle = await this.start(command, { ...options, stdin: options.stdin ?? new Uint8Array() });
    return handle.wait(options);
  }

  async start(command: string[] | string, options: CommandOptions = {}): Promise<Command> {
    return this.#commandSession().start(command, options);
  }

  async writeFile(path: string, data: Uint8Array, options: WriteFileOptions = {}): Promise<void> {
    await this.#commandSession().writeFile(path, data, options);
  }

  async readFile(path: string, options: CallOptions = {}): Promise<Uint8Array> {
    return this.#commandSession().readFile(path, options);
  }

  async end(options: CallOptions = {}): Promise<number> {
    return this.#commandSession().end(options);
  }

  #commandSession(): SandboxSession {
    if (!this.#takesCommands) throw new DvmError("this sandbox runs a supplied program and does not take commands");
    return this.#session ??= new SandboxSession(this.#calls, (options) => this.wait(options), this.#inputOffset);
  }

  /** Raw process output; a command sandbox's stdout is server answer lines. */
  async output(options: CallOptions = {}): Promise<Output> {
    return outputOf(await this.#calls.connection.json("GET", `/executions/${this.id}/output`, options));
  }

  /** `await using` ends input and waits; paused or supplied programs stop. */
  async [Symbol.asyncDispose](): Promise<void> {
    try {
      const state = await this.state();
      if (state.isActive) {
        if (this.#takesCommands && state.kind !== "paused") await this.end();
        else await this.stop();
      }
    } finally { await this.#session?.close(); }
  }

  async #act(action: "pause" | "resume", options: CallOptions): Promise<void> {
    const calls = this.#calls;
    await calls.whileLive(() => calls.connection.json("POST", `${calls.path}/${action}`, options));
  }

  async #waitUntilRunning(run: ApiRun, signal?: AbortSignal): Promise<void> {
    for (;;) {
      const state = new RunState(run.state);
      if (["running", "paused", "exited"].includes(state.kind)) return;
      if (!state.isActive) throw endedErrorOf(this.id, state);
      run = (await this.#newerThan(run, signal)).run;
    }
  }

  #newerThan(run: ApiRun, signal?: AbortSignal): Promise<ApiSandbox> {
    const calls = this.#calls;
    return calls.connection.json<ApiSandbox>("GET", calls.path, {
      query: { newer_than: run.version },
      signal,
    });
  }
}
