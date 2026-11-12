import type { ApiRun, ApiSandbox } from "./api-json.ts";
import { type CallOptions, type ClientOptions, Connection } from "./connection.ts";
import { ConflictError } from "./errors.ts";
import { type Output, outputOf } from "./execution.ts";
import { RunState } from "./run-state.ts";
import { endedErrorOf, SandboxCalls } from "./sandbox-calls.ts";

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

  /** A handle, over `connection`, to the sandbox the API answered with:
   * `Sandbox.create` and `Sandbox.connect` make one. */
  constructor(connection: Connection, sandbox: { id: number; spec: { name: string } }) {
    this.id = sandbox.id;
    this.name = sandbox.spec.name;
    this.#calls = new SandboxCalls(connection, sandbox.id);
  }

  /** A sandbox of `image` (as `docker run` takes it), once it's running.
   * A `signal` that aborts while it starts stops it. */
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
    const sandbox = new Sandbox(connection, created);
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

  async output(options: CallOptions = {}): Promise<Output> {
    return outputOf(await this.#calls.connection.json("GET", `/executions/${this.id}/output`, options));
  }

  /** `await using` stops it at the scope's end. */
  async [Symbol.asyncDispose](): Promise<void> {
    await this.stop();
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
