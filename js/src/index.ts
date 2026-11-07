/** dvm.run's JavaScript SDK: sandboxes whose every run replays.
 *
 * ```ts
 * import { Sandbox, Execution } from "dvm-run";
 *
 * await using sbx = await Sandbox.create("python:3.12");
 * const r = await sbx.commands.run("python -c 'print(1 + 1)'");
 * ```
 */

export {
  type BackgroundOptions,
  CommandHandle,
  type CommandInfo,
  type CommandLine,
  type CommandResult,
  Commands,
  type OutputCallback,
  type RunOptions,
  type StartOptions,
  type WaitOptions,
} from "./commands.ts";
export { type CallOptions, type ClientOptions, NetworkError } from "./connection.ts";
export {
  ApiError,
  AuthenticationError,
  CommandRefusedError,
  ConflictError,
  DvmError,
  InvalidRequestError,
  NotFoundError,
  ReplayEndedError,
  SandboxEndedError,
  SandboxFailedError,
} from "./errors.ts";
export {
  type Described,
  Execution,
  type ExecutionSpec,
  type Moment,
  Replay,
  Verdict,
  type WatchKind,
} from "./execution.ts";
export { Files } from "./files.ts";
export { type RunKind, RunState } from "./run-state.ts";
export { Sandbox, type SandboxOptions } from "./sandbox.ts";
