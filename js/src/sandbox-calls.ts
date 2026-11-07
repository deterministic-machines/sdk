/** A sandbox's calls, which its commands and files make too: its
 * connection, its path, and what a refusal means once it ended. */

import type { ApiSandbox } from "./api-json.ts";
import type { CallOptions, Connection } from "./connection.ts";
import { ConflictError, SandboxEndedError, SandboxFailedError } from "./errors.ts";
import { RunState } from "./run-state.ts";

export class SandboxCalls {
  readonly connection: Connection;
  readonly id: number;
  readonly path: string;

  constructor(connection: Connection, id: number) {
    this.connection = connection;
    this.id = id;
    this.path = `/sandboxes/${id}`;
  }

  async state(options: CallOptions = {}): Promise<RunState> {
    const sandbox = await this.connection.json<ApiSandbox>("GET", this.path, options);
    return new RunState(sandbox.run.state);
  }

  /** What `call` answers; if it's refused because the sandbox ended, that
   * it ended, and how. */
  async whileLive<T>(call: () => Promise<T>): Promise<T> {
    try {
      return await call();
    } catch (refused) {
      if (!(refused instanceof ConflictError)) throw refused;
      const ended = await this.endedError();
      if (ended === null) throw refused;
      ended.cause = refused;
      throw ended;
    }
  }

  /** What a call to it throws if it ended; null if it hasn't. */
  async endedError(): Promise<SandboxEndedError | null> {
    const state = await this.state();
    return state.isActive ? null : endedErrorOf(this.id, state);
  }
}

export function endedErrorOf(sandboxId: number, state: RunState): SandboxEndedError {
  return state.kind === "failed"
    ? new SandboxFailedError(sandboxId, state)
    : new SandboxEndedError(sandboxId, state);
}
