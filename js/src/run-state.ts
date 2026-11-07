/** Where a sandbox, or a replay, is. */

export type RunKind = "queued" | "running" | "paused" | "exited" | "stopped" | "failed";

/** A run's state as the API gives it. */
export interface ApiRunState {
  kind: RunKind;
  code?: number;
  reason?: string;
}

/** `queued`, `running`, `paused`; then `exited` with its `exitCode`,
 * `stopped`, or `failed` with its `reason`. */
export class RunState {
  readonly kind: RunKind;
  readonly exitCode: number | null;
  readonly reason: string | null;

  constructor(state: ApiRunState) {
    this.kind = state.kind;
    this.exitCode = state.code ?? null;
    this.reason = state.reason ?? null;
  }

  /** Whether it hasn't ended. */
  get isActive(): boolean {
    return this.kind === "queued" || this.kind === "running" || this.kind === "paused";
  }
}
