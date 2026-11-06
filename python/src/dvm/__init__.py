"""dvm.run's Python SDK: sandboxes whose every run replays.

    from dvm import Sandbox, Execution

    with Sandbox.create("python:3.12") as sbx:
        r = sbx.commands.run("python -c 'print(1 + 1)'")
    verdict = Execution.get(sbx.id).replay().wait()

``AsyncSandbox`` and ``AsyncExecution`` are the same, for asyncio.
"""

from dvm.blocking import CommandHandle, Commands, Execution, Files, Replay, Sandbox
from dvm.commands import (
    AsyncCommandHandle,
    AsyncCommands,
    CommandInfo,
    CommandLine,
    CommandResult,
)
from dvm.errors import (
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
)
from dvm.execution import AsyncExecution, AsyncReplay, Moment, Verdict
from dvm.files import AsyncFiles
from dvm.run_state import RunState
from dvm.sandbox import AsyncSandbox

__version__ = "0.1.0"

__all__ = [
    "ApiError",
    "AsyncCommandHandle",
    "AsyncCommands",
    "AsyncExecution",
    "AsyncFiles",
    "AsyncReplay",
    "AsyncSandbox",
    "AuthenticationError",
    "CommandHandle",
    "CommandInfo",
    "CommandLine",
    "CommandRefusedError",
    "CommandResult",
    "Commands",
    "ConflictError",
    "DvmError",
    "Execution",
    "Files",
    "InvalidRequestError",
    "Moment",
    "NotFoundError",
    "Replay",
    "ReplayEndedError",
    "RunState",
    "Sandbox",
    "SandboxEndedError",
    "SandboxFailedError",
    "Verdict",
]
