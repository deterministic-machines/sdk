"""What the SDK raises. A program's exit code is never one of these."""

from __future__ import annotations

from dvm.run_state import RunState


class DvmError(Exception):
    """Anything the SDK raises."""


class ApiError(DvmError):
    """dvm.run refused a request, with its status and its reason."""

    def __init__(self, status: int, message: str) -> None:
        super().__init__(f"{status}: {message}")
        self.status = status
        self.message = message


class InvalidRequestError(ApiError):
    """400: the request isn't one dvm.run takes, such as an image that isn't one."""


class AuthenticationError(ApiError):
    """401: the API key was revoked, or isn't one."""


class NotFoundError(ApiError):
    """404: no such sandbox, execution or replay, or not the key's user's."""


class ConflictError(ApiError):
    """409: what was asked can't be done in the state it's in."""


class SandboxEndedError(DvmError):
    """The sandbox ended before the call could act on it."""

    def __init__(self, sandbox_id: int, state: RunState) -> None:
        super().__init__(f"sandbox {sandbox_id} is {state.kind}")
        self.sandbox_id = sandbox_id
        self.state = state


class SandboxFailedError(SandboxEndedError):
    """dvm or the platform ended the sandbox, and why: a syscall dvm can't
    serve, a limit, its runner going away."""

    def __init__(self, sandbox_id: int, state: RunState) -> None:
        DvmError.__init__(self, f"sandbox {sandbox_id} failed: {state.reason}")
        self.sandbox_id = sandbox_id
        self.state = state
        self.reason = state.reason


class ReplayEndedError(DvmError):
    """A replay ended without a verdict: dvm saw it part from its execution
    and failed it, with where, or it was stopped."""

    def __init__(self, replay_id: int, state: RunState) -> None:
        detail = f": {state.reason}" if state.reason else ""
        super().__init__(f"replay {replay_id} {state.kind}{detail}")
        self.replay_id = replay_id
        self.state = state
