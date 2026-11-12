import type { ApiExecution, ApiRun, ApiVerdict, ApiVerdictKind } from "./api-json.ts";
import { type CallOptions, type ClientOptions, Connection } from "./connection.ts";
import { ReplayEndedError } from "./errors.ts";
import { RunState } from "./run-state.ts";

/** What a sandbox was made of. */
export interface ExecutionSpec {
  readonly name: string;
  readonly image: string;
  readonly command: readonly string[];
  readonly entrypoint: string | null;
  readonly cpus: number;
  readonly memoryMib: number;
  readonly seed: number;
  readonly env: Readonly<Record<string, string>>;
}

/** Whether a replay was its execution, as far as can be seen from outside:
 * `same`; `other_exit` (`recorded` and `replayed` codes); `other_output`
 * (the `stream`, and the byte `at` which it parted). */
export class Verdict {
  readonly kind: ApiVerdictKind;
  readonly stream: "stdout" | "stderr" | null;
  readonly at: number | null;
  readonly recorded: number | null;
  readonly replayed: number | null;

  constructor(verdict: ApiVerdict) {
    this.kind = verdict.kind;
    this.stream = verdict.stream ?? null;
    this.at = verdict.at ?? null;
    this.recorded = verdict.recorded ?? null;
    this.replayed = verdict.replayed ?? null;
  }

  get same(): boolean {
    return this.kind === "same";
  }
}

/** A sandbox's execution, from `await Execution.get(sandboxId)`: its spec,
 * its state, its replays, and its log's size once it ended. */
export class Execution {
  readonly id: number;
  readonly spec: ExecutionSpec;
  readonly state: RunState;
  readonly logBytes: number | null;
  readonly replays: readonly Replay[];
  readonly #connection: Connection;

  constructor(connection: Connection, execution: ApiExecution) {
    this.#connection = connection;
    this.id = execution.id;
    const { spec, run } = execution;
    this.spec = {
      name: spec.name,
      image: spec.image,
      command: spec.command,
      entrypoint: spec.entrypoint,
      cpus: spec.cpus,
      memoryMib: spec.memory_mib,
      seed: spec.seed,
      env: Object.fromEntries(spec.environment.map(({ name, value }) => [name, value])),
    };
    this.state = new RunState(run.state);
    this.logBytes = run.log_bytes;
    this.replays = execution.replays.map((replay) => new Replay(connection, replay));
  }

  static async get(executionId: number, options: ClientOptions & CallOptions = {}): Promise<Execution> {
    const connection = await Connection.open(options);
    return new Execution(
      connection,
      await connection.json<ApiExecution>("GET", `/executions/${executionId}`, options),
    );
  }

  /** Replays the exited program from its whole recording. */
  async replay(): Promise<Replay> {
    const replay = await this.#connection.json<ApiRun>("POST", `/executions/${this.id}/replays`);
    return new Replay(this.#connection, replay);
  }
}

/** A replay of an execution. */
export class Replay {
  readonly id: number;
  readonly #connection: Connection;
  readonly #path: string;
  #version = 0;
  #state!: RunState;
  #verdict: Verdict | null = null;

  constructor(connection: Connection, replay: ApiRun) {
    this.#connection = connection;
    this.id = replay.id;
    this.#path = `/replays/${replay.id}`;
    this.#take(replay);
  }

  get state(): RunState {
    return this.#state;
  }

  get verdict(): Verdict | null {
    return this.#verdict;
  }

  /** Its verdict, once it ended: the same execution, or where it parted. A
   * replay dvm failed, as it saw it part, throws where. */
  async wait(options: CallOptions = {}): Promise<Verdict> {
    while (this.#state.isActive) {
      this.#take(
        await this.#connection.json<ApiRun>("GET", this.#path, {
          query: { newer_than: this.#version },
          signal: options.signal,
        }),
      );
    }
    if (this.#verdict === null) throw new ReplayEndedError(this.id, this.#state);
    return this.#verdict;
  }

  async stop(options: CallOptions = {}): Promise<void> {
    this.#take(await this.#connection.json<ApiRun>("POST", `${this.#path}/stop`, options));
  }

  async output(options: CallOptions = {}): Promise<Output> {
    return outputOf(await this.#connection.json("GET", `${this.#path}/output`, options));
  }

  #take(replay: ApiRun): void {
    this.#version = replay.version;
    this.#state = new RunState(replay.state);
    this.#verdict = replay.verdict ? new Verdict(replay.verdict) : null;
  }
}

/** The program's stdout and stderr so far, as bytes. */
export interface Output {
  readonly stdout: Uint8Array;
  readonly stderr: Uint8Array;
}

export function outputOf(output: { stdout: string; stderr: string }): Output {
  const decode = (encoded: string) => Uint8Array.from(atob(encoded), (byte) => byte.charCodeAt(0));
  return { stdout: decode(output.stdout), stderr: decode(output.stderr) };
}
