"""dvm.run's Python SDK: sandboxes with point-in-time inspection.

    from dvm import Sandbox

    with Sandbox.create("python:3.12") as sbx:
        result = sbx.run(["python", "-c", "print(1 + 1)"])
        point = sbx.seek()

``AsyncSandbox`` and ``AsyncSandboxPoint`` are the same, for asyncio.
"""

from dvm.blocking import Command, Sandbox, SandboxPoint
from dvm.errors import (
    ApiError,
    AuthenticationError,
    ConflictError,
    DvmError,
    InvalidRequestError,
    NotFoundError,
    SandboxEndedError,
    SandboxFailedError,
)
from dvm.point import (
    AsyncSandboxPoint,
    DecodedArgument,
    DirEntry,
    FileStat,
    Output,
    StopEvent,
)
from dvm.run_state import RunState
from dvm.sandbox import AsyncSandbox
from dvm.sandbox_session import AsyncCommand, CommandResult

__version__ = "0.3.0"

__all__ = [
    "ApiError",
    "AsyncCommand",
    "AsyncSandbox",
    "AsyncSandboxPoint",
    "AuthenticationError",
    "Command",
    "CommandResult",
    "ConflictError",
    "DecodedArgument",
    "DirEntry",
    "DvmError",
    "FileStat",
    "InvalidRequestError",
    "NotFoundError",
    "Output",
    "RunState",
    "Sandbox",
    "SandboxEndedError",
    "SandboxFailedError",
    "SandboxPoint",
    "StopEvent",
]
