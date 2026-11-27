"""One sandbox-server session over the platform's input and output bytes."""

from __future__ import annotations

import asyncio
from dataclasses import dataclass
from typing import Any, Awaitable, Callable, Mapping, Optional

from dvm.connection import Connection
from dvm.errors import ConflictError, DvmError, SandboxEndedError
from dvm.run_state import RunState
from dvm.frame import FrameDecoder, MAX_HEADER, decode_output, encode_frame


@dataclass(frozen=True)
class CommandResult:
    """A command's bytes and exit code (128 + signal when killed).

    A nonzero code is a result. The protocol does not distinguish an
    explicit exit with that code from a signal.
    """

    exit_code: int
    stdout: bytes
    stderr: bytes


class _Operation:
    def __init__(self, request_id: int) -> None:
        self.id = request_id
        self.started = asyncio.Event()
        self.done = asyncio.Event()
        self.error: Optional[BaseException] = None
        self.stdout = bytearray()
        self.stderr = bytearray()
        self.data = bytearray()
        self.result: Optional[CommandResult] = None

    def check(self) -> None:
        if self.error is not None:
            raise self.error

    def fail(self, error: BaseException) -> None:
        self.error = error
        self.started.set()
        self.done.set()

    async def wait(self) -> None:
        await self.done.wait()
        self.check()


class AsyncCommand:
    """A started command. Its stdin stays open until closed, unless bytes
    were supplied to ``start``. Signals address the whole process group.
    """

    def __init__(self, session: SandboxSession, operation: _Operation) -> None:
        self._session = session
        self._operation = operation
        self._stdin_closed = False

    async def wait(self) -> CommandResult:
        """The command's result; a nonzero exit code does not raise."""
        await self._operation.wait()
        assert self._operation.result is not None
        return self._operation.result

    async def signal(self, number: int) -> None:
        """Sends a signal; the protocol has no success acknowledgment."""
        if not 1 <= number <= 64:
            raise DvmError("signal must be between 1 and 64")
        await self._session.send(self._operation, {"signal": number})

    async def write_stdin(self, data: bytes) -> None:
        """Queues bytes for the command, without waiting for it to read."""
        if self._stdin_closed:
            raise DvmError("command stdin is closed")
        await self._session.send_data(self._operation, "stdin", {}, data)

    async def close_stdin(self) -> None:
        """Closes stdin after its queued bytes; safe to call again."""
        if not self._stdin_closed:
            await self._session.send(self._operation, {"stdin": {"close": True}})
            self._stdin_closed = True


class SandboxSession:
    PIECE_SIZE = 1024 * 1024

    def __init__(
        self,
        connection: Connection,
        sandbox_id: int,
        wait: Callable[[], Awaitable[int]],
        input_offset: Optional[int],
        ended_error: Callable[[], Awaitable[Optional[SandboxEndedError]]],
    ) -> None:
        self._connection = connection
        self._id = sandbox_id
        self._wait = wait
        self._ended_error = ended_error
        self._input_offset = input_offset
        self._output_offset = 0
        self._stderr_offset = 0
        self._decoder = FrameDecoder()
        self._operations: dict[int, _Operation] = {}
        self._writer = asyncio.Lock()
        self._reader: Optional[asyncio.Task[None]] = None
        self._error: Optional[BaseException] = None
        self._ended = False

    async def start(
        self, command: list[str] | str, *, env: Optional[Mapping[str, str]],
        cwd: Optional[str], stdin: Optional[bytes], user: Optional[str] = None,
    ) -> AsyncCommand:
        run: dict[str, Any] = {
            "script" if isinstance(command, str) else "argv": command,
            "stdin": True,
        }
        if env is not None:
            run["env"] = dict(env)
        if cwd is not None:
            run["cwd"] = cwd
        if user is not None:
            run["user"] = user
        operation = await self._begin({"run": run})
        await operation.started.wait()
        operation.check()
        handle = AsyncCommand(self, operation)
        if stdin is not None:
            if stdin:
                await handle.write_stdin(stdin)
            await handle.close_stdin()
        return handle

    async def write_file(self, path: str, data: bytes, mode: Optional[int]) -> None:
        fields: dict[str, Any] = {"path": path}
        if mode is not None:
            fields["mode"] = mode
        async with self._writer:
            await self._ready()
            operation = self._register()
            try:
                for action, payload in self._pieces("write", fields, data):
                    operation.check()
                    await self._post(operation.id, action, payload)
            except BaseException:
                self._operations.pop(operation.id, None)
                raise
        await operation.wait()

    async def read_file(self, path: str) -> bytes:
        operation = await self._begin({"read": {"path": path}})
        await operation.wait()
        return bytes(operation.data)

    async def end(self) -> int:
        async with self._writer:
            if not self._ended:
                await self._ready(ending=True)
                await self._append(b"", end=True)
                self._ended = True
        return await self._wait()

    async def close(self) -> None:
        self._fail(DvmError("sandbox connection is closed"))
        if self._reader is not None:
            self._reader.cancel()
            try:
                await self._reader
            except asyncio.CancelledError:
                pass

    async def send(self, operation: _Operation, action: dict[str, Any]) -> None:
        async with self._writer:
            await self._ready()
            operation.check()
            await self._post(operation.id, action)

    async def send_data(
        self, operation: _Operation, kind: str, fields: dict[str, Any], data: bytes,
    ) -> None:
        async with self._writer:
            await self._ready()
            for action, payload in self._pieces(kind, fields, data):
                operation.check()
                await self._post(operation.id, action, payload)

    async def _begin(self, action: dict[str, Any]) -> _Operation:
        async with self._writer:
            await self._ready()
            operation = self._register()
            try:
                await self._post(operation.id, action)
            except BaseException:
                self._operations.pop(operation.id, None)
                raise
            return operation

    def _register(self) -> _Operation:
        assert self._input_offset is not None
        operation = _Operation(self._input_offset)
        self._operations[operation.id] = operation
        return operation

    async def _ready(self, *, ending: bool = False) -> None:
        if self._error is not None and not ending:
            raise self._error
        if self._ended:
            raise DvmError("sandbox input has ended")
        if self._input_offset is None:
            await self._append(b"")
        if self._reader is None:
            self._reader = asyncio.create_task(self._read_answers())

    async def _append(self, chunk: bytes, *, end: bool = False) -> None:
        try:
            state = await self._connection.input(
                f"/sandboxes/{self._id}/input", chunk,
                offset=self._input_offset or 0, end=end,
            )
        except ConflictError as refused:
            ended = await self._ended_error()
            if ended is not None:
                raise ended from refused
            raise
        self._input_offset = state["length"]

    async def _post(self, request_id: int, action: dict[str, Any], payload: bytes = b"") -> None:
        header = {"id": request_id, **action}
        frame = encode_frame(header, payload)
        if len(frame) - len(payload) - 1 > MAX_HEADER:
            raise DvmError("request header exceeds 65536 bytes")
        for at in range(0, len(frame), 64 * 1024):
            await self._append(frame[at:at + 64 * 1024])

    def _pieces(self, kind: str, fields: dict[str, Any], data: bytes):
        for at in range(0, max(1, len(data)), self.PIECE_SIZE):
            piece = data[at:at + self.PIECE_SIZE]
            body = dict(fields)
            if kind == "write":
                body["last"] = at + len(piece) == len(data)
            yield {kind: body}, piece

    async def _read_answers(self) -> None:
        try:
            while True:
                body = await self._connection.bytes(
                    f"/sandboxes/{self._id}/output",
                    params={"offset": self._output_offset, "stderr_offset": self._stderr_offset},
                )
                page = decode_output(body)
                self._output_offset += len(page.stdout)
                self._stderr_offset += len(page.stderr)
                self._decoder.push(page.stdout)
                while (frame := self._decoder.next_frame()) is not None:
                    self._dispatch(frame.header, frame.payload)
                if not page.more and not page.cut:
                    code = await self._wait()
                    raise SandboxEndedError(self._id, RunState("exited", exit_code=code))
        except Exception as error:
            self._fail(error)

    def _dispatch(self, answer: dict[str, Any], payload: bytes) -> None:
        if "error" in answer and "id" not in answer:
            self._fail_operations(DvmError(answer["error"]))
            return
        operation = self._operations.get(answer.get("id"))
        if operation is None:
            return
        if "error" in answer:
            operation.fail(DvmError(answer["error"]))
        elif "started" in answer:
            operation.started.set()
        elif "stdout" in answer:
            operation.stdout.extend(payload)
        elif "stderr" in answer:
            operation.stderr.extend(payload)
        elif "exited" in answer:
            operation.result = CommandResult(
                answer["exited"]["code"], bytes(operation.stdout), bytes(operation.stderr),
            )
            operation.done.set()
        elif "data" in answer:
            operation.data.extend(payload)
        elif "written" in answer or "end" in answer:
            operation.done.set()
        if operation.done.is_set():
            self._operations.pop(operation.id, None)

    def _fail(self, error: BaseException) -> None:
        if self._error is None:
            self._error = error
        self._fail_operations(self._error)

    def _fail_operations(self, error: BaseException) -> None:
        for operation in self._operations.values():
            operation.fail(error)
        self._operations.clear()
