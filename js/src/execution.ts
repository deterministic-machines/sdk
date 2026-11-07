/** An execution: everything a sandbox ran, one log from its start to its
 * end, under the sandbox's id; and its replays, each judged the same
 * execution or not. */

import type { ApiCommand, ApiExecution, ApiMoment, ApiRun, ApiVerdict, ApiVerdictKind } from "./api-json.ts";
import { type CommandInfo, commandInfoOf } from "./commands.ts";
import { type CallOptions, type ClientOptions, Connection } from "./connection.ts";
import { ReplayEndedError } from "./errors.ts";
import { RunState } from "./run-state.ts";

/** A moment of the execution: `"<thread>:<operation>"`, before that
 * thread's operation, or `"@<milliseconds>"` into it, as dvm writes them. */
export type Moment = string | ApiMoment;

/** What a replay may watch. */
export type WatchKind =
  | "files"
  | "network"
  | "processes"
  | "memory"
  | "time"
  | "signals"
  | "waits"
  | "other"
  | "page-faults";

/** dvm's description of a machine, or of what a replay saw, as the API
 * gives it. */
export type Described = Record<string, unknown>;

/** What a sandbox was made of. */
export interface ExecutionSpec {
  readonly name: string;
  readonly image: string;
  readonly cpus: number;
  readonly memoryMib: number;
  readonly seed: number;
  readonly env: Readonly<Record<string, string>>;
}

/** Whether a replay was its execution, as far as can be seen from outside:
 * `same`; `other_exit` (`recorded` and `replayed` codes); `other_output`
 * (the `stream`, and the byte `at` which it parted); or `other_command`.
 * `command` is the command it parted in. */
export class Verdict {
  readonly kind: ApiVerdictKind;
  readonly command: number | null;
  readonly stream: "stdout" | "stderr" | null;
  readonly at: number | null;
  readonly recorded: number | null;
  readonly replayed: number | null;

  constructor(verdict: ApiVerdict) {
    this.kind = verdict.kind;
    this.command = verdict.command ?? null;
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
 * its state, each command it was sent, as it ran, and its log's size once
 * it ended. */
export class Execution {
  readonly id: number;
  readonly spec: ExecutionSpec;
  readonly state: RunState;
  readonly logBytes: number | null;
  readonly commands: readonly CommandInfo[];
  readonly replays: readonly Replay[];
  readonly #connection: Connection;

  constructor(connection: Connection, execution: ApiExecution) {
    this.#connection = connection;
    this.id = execution.id;
    const { spec, run } = execution;
    this.spec = {
      name: spec.name,
      image: spec.image,
      cpus: spec.cpus,
      memoryMib: spec.memory_mib,
      seed: spec.seed,
      env: Object.fromEntries(spec.environment.map(({ name, value }) => [name, value])),
    };
    this.state = new RunState(run.state);
    this.logBytes = run.log_bytes;
    this.commands = execution.commands.map(commandInfoOf);
    this.replays = execution.replays.map((replay) => new Replay(connection, replay));
  }

  static async get(executionId: number, options: ClientOptions & CallOptions = {}): Promise<Execution> {
    const connection = await Connection.open(options);
    return new Execution(
      connection,
      await connection.json<ApiExecution>("GET", `/executions/${executionId}`, options),
    );
  }

  /** Replays it, once its sandbox ended, played to its end, watching the
   * events `watch` names; or `held` at its start, for `seek`. */
  async replay(options: CallOptions & { watch?: WatchKind[]; held?: boolean } = {}): Promise<Replay> {
    const body = { held: options.held ?? false, watch: options.watch ?? [] };
    const replay = await this.#connection.json<ApiRun>("POST", `/executions/${this.id}/replays`, {
      body,
      signal: options.signal,
    });
    return new Replay(this.#connection, replay);
  }

  /** The machine at the moment `at`: each process, its threads, files and
   * memory, as dvm describes them. */
  machine(at: Moment, options: CallOptions = {}): Promise<Described> {
    const query = { at: typeof at === "string" ? at : momentText(at) };
    return this.#connection.json("GET", `/executions/${this.id}/machine`, { query, signal: options.signal });
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

  /** Takes a held replay to the moment `to`, and holds it there: the
   * moment, each thread's next operation, and the machine there. */
  seek(to: Moment, options: CallOptions = {}): Promise<Described> {
    return this.#connection.json("POST", `${this.#path}/seek`, {
      body: { to: momentOf(to) },
      signal: options.signal,
    });
  }

  async stop(options: CallOptions = {}): Promise<void> {
    this.#take(await this.#connection.json<ApiRun>("POST", `${this.#path}/stop`, options));
  }

  /** The commands it ran again, oldest first. */
  async commands(options: CallOptions = {}): Promise<CommandInfo[]> {
    const commands = await this.#connection.json<ApiCommand[]>("GET", `${this.#path}/commands`, options);
    return commands.map(commandInfoOf);
  }

  /** What it watched, and the events it saw, once it ended. */
  events(options: CallOptions = {}): Promise<Described> {
    return this.#connection.json("GET", `${this.#path}/events`, options);
  }

  #take(replay: ApiRun): void {
    this.#version = replay.version;
    this.#state = new RunState(replay.state);
    this.#verdict = replay.verdict ? new Verdict(replay.verdict) : null;
  }
}

/** A moment as dvm writes it, as the API takes it. */
export function momentOf(moment: Moment): ApiMoment {
  if (typeof moment !== "string") return moment;
  if (moment.startsWith("@")) {
    const [whole = "0", fraction = ""] = moment.slice(1).split(".");
    return { kind: "time", at_us: Number(whole) * 1000 + Number(`${fraction}000`.slice(0, 3)) };
  }
  const [thread = "", operation = ""] = moment.split(":");
  return { kind: "event", thread: Number(thread), operation: Number(operation) };
}

/** A moment as dvm writes it. */
function momentText(moment: ApiMoment): string {
  if (moment.kind === "event") return `${moment.thread}:${moment.operation}`;
  const fraction = String(moment.at_us % 1000).padStart(3, "0");
  return `@${Math.floor(moment.at_us / 1000)}.${fraction}`;
}
