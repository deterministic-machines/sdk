"""Commands in a sandbox: run one and get its output, or start one in the
background and hold a handle to it."""

from __future__ import annotations

import asyncio
import base64
import codecs
from contextlib import aclosing
from dataclasses import dataclass
from typing import TYPE_CHECKING, Any, Callable, Mapping, Optional, Sequence, Union

import httpx

from dvm.errors import CommandRefusedError, DvmError

if TYPE_CHECKING:
    from dvm.sandbox import AsyncSandbox

#: A script, which the sandbox's shell runs (``/bin/bash -lc`` if the image
#: has bash, else ``/bin/sh -c``), or an argv, which runs as it is.
CommandLine = Union[str, Sequence[str]]

#: Called with each piece of a stream's output, as text, as it's written.
OutputCallback = Callable[[str], Any]

_SIGKILL = 9


@dataclass(frozen=True)
class CommandResult:
    """A command that exited: its output, and its exit code. A signal's kill
    is code 128 and the signal, as a shell says, with the signal given."""

    number: int
    stdout: str
    stderr: str
    exit_code: int
    signal: Optional[int] = None


@dataclass(frozen=True)
class CommandInfo:
    """A command as its execution recorded it: ``state`` is ``starting``,
    ``running``, ``exited`` (with ``exit_code``), ``refused`` (with
    ``reason``), or ``unfinished`` as its sandbox ended first. Times are
    milliseconds since the Unix epoch."""

    number: int
    command: CommandLine
    argv: Optional[list[str]]
    state: str
    exit_code: Optional[int]
    signal: Optional[int]
    reason: Optional[str]
    started_at: Optional[int]
    ended_at: Optional[int]

    @classmethod
    def from_api(cls, command: Mapping[str, Any]) -> CommandInfo:
        state = command["state"]
        return cls(
            number=command["number"],
            command=command["command"],
            argv=command["argv"],
            state=state["kind"],
            exit_code=state.get("code"),
            signal=state.get("signal"),
            reason=state.get("reason"),
            started_at=command["started_at"],
            ended_at=command["ended_at"],
        )

    @property
    def is_active(self) -> bool:
        """Whether it may still start, or exit."""
        return self.state in ("starting", "running")


class AsyncCommands:
    """A sandbox's commands: ``sandbox.commands``."""

    def __init__(self, sandbox: AsyncSandbox) -> None:
        self._sandbox = sandbox

    async def run(
        self,
        command: CommandLine,
        *,
        background: bool = False,
        cwd: Optional[str] = None,
        env: Optional[Mapping[str, str]] = None,
        stdin: Union[bool, bytes, str] = False,
        timeout: Optional[float] = None,
        on_stdout: Optional[OutputCallback] = None,
        on_stderr: Optional[OutputCallback] = None,
    ) -> Union[CommandResult, AsyncCommandHandle]:
        """Runs ``command`` to its exit: its :class:`CommandResult`. With
        ``background=True``, a handle to it at once instead, which takes
        ``timeout`` and the callbacks in its ``wait``.

        ``stdin=True`` keeps its stdin open for ``send_stdin``; bytes or text
        are sent to it, and it's closed after. Else it reads ``/dev/null``.
        ``timeout`` is in seconds: once it passes, the command is killed
        (SIGKILL), and its result says so.
        """
        if background and (timeout is not None or on_stdout or on_stderr):
            raise TypeError("a background command takes timeout= and callbacks in wait()")
        new: dict[str, Any] = {
            "command": command if isinstance(command, str) else list(command),
        }
        if cwd is not None:
            new["cwd"] = cwd
        if env:
            new["env"] = dict(env)
        if stdin is not False:
            new["stdin"] = True
        sandbox = self._sandbox
        started = await sandbox._while_live(
            sandbox._connection.json("POST", f"{sandbox._path}/commands", body=new)
        )
        handle = AsyncCommandHandle(sandbox, started["number"])
        if isinstance(stdin, (bytes, str)):
            await handle.send_stdin(stdin, close=True)
        if background:
            return handle
        return await handle.wait(timeout=timeout, on_stdout=on_stdout, on_stderr=on_stderr)


class AsyncCommandHandle:
    """A command started in the background."""

    def __init__(self, sandbox: AsyncSandbox, number: int) -> None:
        self._sandbox = sandbox
        self.number = number
        self._path = f"{sandbox._path}/commands/{number}"

    async def wait(
        self,
        *,
        timeout: Optional[float] = None,
        on_stdout: Optional[OutputCallback] = None,
        on_stderr: Optional[OutputCallback] = None,
    ) -> CommandResult:
        """Its result once it exits, with all it wrote. The callbacks get its
        output from its start, as it's written."""
        followed = _FollowedOutput(self._sandbox, self._path, on_stdout, on_stderr)
        try:
            await asyncio.wait_for(followed.follow(), timeout)
        except asyncio.TimeoutError:
            await self.kill()
            await followed.follow()
        return await followed.result()

    async def send_stdin(self, data: Union[bytes, str], *, close: bool = False) -> None:
        """Sends ``data`` to its stdin, which it was started with open."""
        content = data.encode() if isinstance(data, str) else data
        params = {"close": "true"} if close else None
        await self._sandbox._while_live(
            self._sandbox._connection.bytes(
                "POST", f"{self._path}/stdin", params=params, content=content
            )
        )

    async def close_stdin(self) -> None:
        """Closes its stdin: it reads no more than it was sent."""
        await self.send_stdin(b"", close=True)

    async def signal(self, signal: int) -> None:
        """Sends ``signal`` to its process (not what that started)."""
        await self._sandbox._while_live(
            self._sandbox._connection.json(
                "POST", f"{self._path}/signal", body={"signal": signal}
            )
        )

    async def kill(self) -> None:
        """Sends it SIGKILL."""
        await self.signal(_SIGKILL)

    async def info(self) -> CommandInfo:
        """The command as it is now."""
        command = await self._sandbox._connection.json("GET", self._path)
        return CommandInfo.from_api(command)


class _FollowedOutput:
    """A command's output, followed from its start to its end over
    server-sent events; a stream that drops is followed again from where it
    got to."""

    # Streams in a row that end before the command, at once and with nothing
    # new: one that went on a while was cut, by the network or a proxy.
    _MOST_FRUITLESS_FOLLOWS = 3
    _CUT_AFTER_SECONDS = 1.0

    def __init__(
        self,
        sandbox: AsyncSandbox,
        path: str,
        on_stdout: Optional[OutputCallback],
        on_stderr: Optional[OutputCallback],
    ) -> None:
        self._sandbox = sandbox
        self._path = f"{path}/output"
        self._streams = {
            "stdout": _StreamOutput(on_stdout),
            "stderr": _StreamOutput(on_stderr),
        }
        self._command: Optional[CommandInfo] = None

    async def follow(self) -> None:
        """Returns once the command ended."""
        loop = asyncio.get_running_loop()
        fruitless = 0
        while self._command is None or self._command.is_active:
            written = self._written()
            started = loop.time()
            dropped: Optional[httpx.TransportError] = None
            try:
                await self._follow_from_offsets()
            except httpx.TransportError as error:
                dropped = error
            if self._written() > written or loop.time() - started > self._CUT_AFTER_SECONDS:
                fruitless = 0
            else:
                fruitless += 1
            if fruitless > self._MOST_FRUITLESS_FOLLOWS:
                raise DvmError(f"{self._path}'s stream ended before its command") from dropped

    async def _follow_from_offsets(self) -> None:
        params = {
            "follow": "true",
            "stdout_offset": len(self._streams["stdout"].written),
            "stderr_offset": len(self._streams["stderr"].written),
        }
        events = self._sandbox._connection.events(self._path, params=params)
        async with aclosing(events):
            async for event in events:
                self._take(event)

    def _written(self) -> int:
        return sum(len(stream.written) for stream in self._streams.values())

    def _take(self, event: Mapping[str, Any]) -> None:
        if event["kind"] == "command":
            self._command = CommandInfo.from_api(event["command"])
            return
        chunk = base64.b64decode(event["chunk"])
        self._streams[event["stream"]].take(event["offset"], chunk)

    async def result(self) -> CommandResult:
        command = self._command
        assert command is not None and not command.is_active
        if command.state == "refused":
            raise CommandRefusedError(command.number, command.reason or "")
        if command.state == "unfinished":
            ended = await self._sandbox._ended_error()
            raise ended or DvmError(f"command {command.number} is unfinished")
        assert command.exit_code is not None
        return CommandResult(
            number=command.number,
            stdout=self._streams["stdout"].finish(),
            stderr=self._streams["stderr"].finish(),
            exit_code=command.exit_code,
            signal=command.signal,
        )


class _StreamOutput:
    """One stream's bytes so far, and its text to a callback as it comes:
    a character split across chunks waits for its end."""

    def __init__(self, callback: Optional[OutputCallback]) -> None:
        self.written = bytearray()
        self._callback = callback
        self._decoder = codecs.getincrementaldecoder("utf-8")("replace")

    def take(self, offset: int, chunk: bytes) -> None:
        if offset > len(self.written):
            raise DvmError(f"output from byte {offset}, after only {len(self.written)}")
        new = chunk[len(self.written) - offset :]
        self.written += new
        if self._callback and new:
            text = self._decoder.decode(new)
            if text:
                self._callback(text)

    def finish(self) -> str:
        if self._callback:
            text = self._decoder.decode(b"", final=True)
            if text:
                self._callback(text)
        return self.written.decode("utf-8", "replace")
