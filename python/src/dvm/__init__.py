"""dvm.run's Python SDK: sandboxes whose every run replays.

    from dvm import Sandbox, Execution

    with Sandbox.create("python:3.12") as sbx:
        result = sbx.run(["python", "-c", "print(1 + 1)"])
    verdict = Execution.get(sbx.id).replay().wait()

``AsyncSandbox`` and ``AsyncExecution`` are the same, for asyncio.
"""

from dvm.blocking import Command, Execution, Replay, Sandbox
from dvm.errors import (
    ApiError,
    AuthenticationError,
    ConflictError,
    DvmError,
    InvalidRequestError,
    NotFoundError,
    ReplayEndedError,
    SandboxEndedError,
    SandboxFailedError,
)
from dvm.execution import AsyncExecution, AsyncReplay, Output, Verdict
from dvm.run_state import RunState
from dvm.sandbox import AsyncSandbox
from dvm.sandbox_session import AsyncCommand, CommandResult

__version__ = "0.2.0"

__all__ = [
    "ApiError",
    "AsyncCommand",
    "Command",
    "CommandResult",
    "AsyncExecution",
    "AsyncReplay",
    "AsyncSandbox",
    "AuthenticationError",
    "ConflictError",
    "DvmError",
    "Execution",
    "InvalidRequestError",
    "NotFoundError",
    "Output",
    "Replay",
    "ReplayEndedError",
    "RunState",
    "Sandbox",
    "SandboxEndedError",
    "SandboxFailedError",
    "Verdict",
]
