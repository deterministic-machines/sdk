/** What the SDK throws. A command's exit code is never one of these. */

import type { RunState } from "./run-state.ts";

/** Anything the SDK throws. */
export class DvmError extends Error {
  constructor(message: string, options?: ErrorOptions) {
    super(message, options);
    this.name = new.target.name;
  }
}

/** dvm.run refused a request: its status, and its reason. */
export class ApiError extends DvmError {
  readonly status: number;
  readonly reason: string;

  constructor(status: number, reason: string) {
    super(`${status}: ${reason}`);
    this.status = status;
    this.reason = reason;
  }
}

/** 400: the request isn't one dvm.run takes, such as an image that isn't one. */
export class InvalidRequestError extends ApiError {}

/** 401: the API key was revoked, or isn't one. */
export class AuthenticationError extends ApiError {}

/** 404: no such sandbox, command, execution or replay, or not the key's user's. */
export class NotFoundError extends ApiError {}

/** 409: what was asked can't be done in the state it's in. */
export class ConflictError extends ApiError {}

/** The sandbox ended before the call could act on it. */
export class SandboxEndedError extends DvmError {
  readonly sandboxId: number;
  readonly state: RunState;

  constructor(sandboxId: number, state: RunState, message = `sandbox ${sandboxId} is ${state.kind}`) {
    super(message);
    this.sandboxId = sandboxId;
    this.state = state;
  }
}

/** dvm or the platform ended the sandbox, and why: a syscall dvm can't
 * serve, a limit, its runner going away. */
export class SandboxFailedError extends SandboxEndedError {
  readonly reason: string;

  constructor(sandboxId: number, state: RunState) {
    super(sandboxId, state, `sandbox ${sandboxId} failed: ${state.reason}`);
    this.reason = state.reason ?? "";
  }
}

/** The sandbox didn't take the command, and why: a script in an image with
 * no shell, a command too long. */
export class CommandRefusedError extends DvmError {
  readonly number: number;
  readonly reason: string;

  constructor(number: number, reason: string) {
    super(`command ${number} was refused: ${reason}`);
    this.number = number;
    this.reason = reason;
  }
}

/** A replay ended without a verdict: dvm saw it part from its execution and
 * failed it, with where, or it was stopped. */
export class ReplayEndedError extends DvmError {
  readonly replayId: number;
  readonly state: RunState;

  constructor(replayId: number, state: RunState) {
    const detail = state.reason ? `: ${state.reason}` : "";
    super(`replay ${replayId} ${state.kind}${detail}`);
    this.replayId = replayId;
    this.state = state;
  }
}
