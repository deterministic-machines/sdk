"""Session ordering over fake byte streams, without clocks or sleeps."""
import asyncio

import httpx
import pytest

from dvm import AsyncSandbox, DvmError, SandboxEndedError, SandboxFailedError
from dvm.connection import Connection
from dvm.frame import FrameDecoder, encode_frame


def line(answer):
    answer = dict(answer)
    payload = answer.pop("payload", b"")
    return encode_frame(answer, payload)


class Streams:
    def __init__(self, *, length=0, state=None):
        self.decoder = FrameDecoder()
        self.base = length
        self.length = length
        self.state = state or {"kind": "exited", "code": 0}
        self.requests = asyncio.Queue()
        self.chunks = asyncio.Queue()
        self.output_offset = 0
        self.probes = 0
        connection = Connection("test", "http://dvm.test", transport=httpx.MockTransport(self.answer))
        self.sandbox = AsyncSandbox(connection, {"id": 5, "spec": {"name": "test"}})

    async def answer(self, request):
        if request.method == "POST":
            assert request.headers["content-type"] == "application/octet-stream"
            data = request.content
            offset = int(request.url.params["offset"])
            end = request.url.params["end"] == "true"
            if not data:
                assert offset == 0
                self.probes += 1
            else:
                assert offset == self.length
                assert len(data) <= 65536
                self.length += len(data)
                self.decoder.push(data)
                while (frame := self.decoder.next_frame()) is not None:
                    assert len(frame.payload) <= 1024 * 1024
                    request = frame.header
                    if frame.payload:
                        request["payload"] = frame.payload
                    await self.requests.put((self.base + frame.span.start, request))
            return httpx.Response(200, json={"length": self.length, "ended": end})
        if request.url.path.endswith("/output"):
            assert int(request.url.params["offset"]) == self.output_offset
            chunk, more = await self.chunks.get()
            self.output_offset += len(chunk)
            return httpx.Response(200, content=encode_frame({"stdout": len(chunk), "stderr": 0, "more": more, "cut": False}, chunk),
                                  headers={"Content-Type": "application/octet-stream"})
        return httpx.Response(200, json={"run": {"state": self.state}})

    async def emit(self, *answers, more=True):
        await self.chunks.put((b"".join(map(line, answers)), more))

    async def start(self, name):
        pending = asyncio.create_task(self.sandbox.start(name))
        offset, request = await self.requests.get()
        assert request["id"] == offset
        await self.emit({"id": offset, "started": {"pid": 1, "argv": [name]}})
        return await pending, offset


def test_split_lines_and_interleaved_commands_use_the_input_offsets():
    async def run():
        streams = Streams(length=123)
        first = asyncio.create_task(streams.sandbox.start(["first"]))
        first_id, request = await streams.requests.get()
        assert first_id == request["id"] == 123
        assert "user" not in request["run"]
        second = asyncio.create_task(streams.sandbox.start("second", user="agent"))
        second_id, request = await streams.requests.get()
        assert second_id == request["id"] > first_id
        assert request["run"]["script"] == "second"
        assert request["run"]["user"] == "agent"
        answers = (line({"id": first_id, "started": {"pid": 1, "argv": ["first"]}})
                   + line({"id": second_id, "started": {"pid": 2, "argv": ["sh", "-c", "second"]}})
                   + line({"id": first_id, "stdout": True, "payload": b"a"}))
        await streams.chunks.put((answers[:-4], True))
        await streams.chunks.put((answers[-4:], True))
        a, b = await asyncio.gather(first, second)
        data = bytes(range(256)) * 9000
        writing = asyncio.create_task(a.write_stdin(data))
        received = bytearray()
        while len(received) < len(data):
            _, request = await streams.requests.get()
            assert request["id"] == first_id
            received.extend(request["payload"])
        await writing
        assert received == data
        await a.close_stdin()
        _, request = await streams.requests.get()
        assert request == {"id": first_id, "stdin": {"close": True}}
        await streams.emit({"id": second_id, "stdout": True, "payload": b"b"},
                           {"id": first_id, "stderr": True, "payload": b"err"},
                           {"id": second_id, "exited": {"code": 3}},
                           {"id": first_id, "exited": {"code": 0}})
        ra, rb = await asyncio.gather(a.wait(), b.wait())
        assert (ra.exit_code, ra.stdout, ra.stderr) == (0, b"a", b"err")
        assert (rb.exit_code, rb.stdout, rb.stderr) == (3, b"b", b"")
        assert streams.probes == 1
        await streams.sandbox.close()
    asyncio.run(run())


@pytest.mark.parametrize("failure", ["id", "no-id", "exit", "failed"])
def test_errors_dispatch_to_one_or_all_open_operations(failure):
    async def run():
        state = {"kind": "failed", "reason": "recording limit"} if failure == "failed" else {"kind": "exited", "code": 7}
        streams = Streams(state=state)
        a, aid = await streams.start("first")
        b, bid = await streams.start("second")
        if failure == "id":
            await streams.emit({"id": aid, "error": "refused command"},
                               {"id": bid, "exited": {"code": 0}})
            with pytest.raises(DvmError, match="refused command"):
                await a.wait()
            assert (await b.wait()).exit_code == 0
        else:
            if failure == "no-id":
                await streams.emit({"error": "bad line"})
                error, message = DvmError, "bad line"
            else:
                await streams.emit(more=False)
                error = SandboxFailedError if failure == "failed" else SandboxEndedError
                message = "recording limit" if failure == "failed" else "exited"
            for command in (a, b):
                with pytest.raises(error, match=message):
                    await command.wait()
            if failure == "no-id":
                following, fid = await streams.start("following")
                await streams.emit({"id": fid, "exited": {"code": 0}})
                assert (await following.wait()).exit_code == 0
        await streams.sandbox.close()
    asyncio.run(run())


def test_files_are_cut_into_bounded_pieces_and_read_back():
    async def run():
        streams = Streams()
        data = bytes(range(256)) * 9000
        writing = asyncio.create_task(streams.sandbox.write_file("/tmp/a", data, mode=0o600))
        received = bytearray()
        write_id = None
        while True:
            _, request = await streams.requests.get()
            if write_id is None:
                write_id = request["id"]
            assert request["id"] == write_id
            piece = request["write"]
            assert (piece["path"], piece["mode"]) == ("/tmp/a", 0o600)
            received.extend(request["payload"])
            if piece["last"]:
                break
        assert received == data
        await streams.emit({"id": write_id, "written": True})
        await writing
        reading = asyncio.create_task(streams.sandbox.read_file("/tmp/a"))
        read_id, request = await streams.requests.get()
        assert request == {"id": read_id, "read": {"path": "/tmp/a"}}
        assert read_id > write_id
        for at in range(0, len(data), 8000):
            await streams.emit({"id": read_id, "data": True, "payload": data[at:at + 8000]})
        await streams.emit({"id": read_id, "end": True})
        assert await reading == data
        await streams.sandbox.close()
    asyncio.run(run())


def test_program_sandboxes_refuse_locally_and_input_backpressure_is_not_retried():
    async def run():
        calls = []
        def answer(request):
            calls.append(request)
            if request.method == "POST":
                return httpx.Response(409, json={"message": "the program is not reading its input; input length 0"})
            return httpx.Response(200, json={"run": {"state": {"kind": "running"}}})
        connection = Connection("test", "http://dvm.test", transport=httpx.MockTransport(answer))
        program = AsyncSandbox(connection, {"id": 5, "spec": {"name": "test", "command": ["cat"]}})
        for call in (program.run("true"), program.start("true"), program.write_file("a", b""),
                     program.read_file("a"), program.end()):
            with pytest.raises(DvmError, match="supplied program"):
                await call
        assert not calls
        session = AsyncSandbox(connection, {"id": 5, "spec": {"name": "test"}}, input_offset=0)
        with pytest.raises(DvmError, match="not reading its input"):
            await session.run("true")
        assert sum(request.method == "POST" for request in calls) == 1
        await session.close()
    asyncio.run(run())


@pytest.mark.parametrize("kind", ["session", "paused", "program", "explicit"])
def test_scope_cleanup_ends_input_or_stops_as_appropriate(kind):
    async def run():
        calls = []
        state = {"kind": "paused" if kind == "paused" else "running"}
        async def answer(request):
            nonlocal state
            if request.method == "GET" and request.url.path.endswith("/output"):
                return httpx.Response(200, content=encode_frame({"stdout": 0, "stderr": 0, "more": False, "cut": False}))
            if request.method == "GET":
                return httpx.Response(200, json={"run": {"state": state}})
            calls.append(request.url.path)
            if request.url.path.endswith("/input"):
                assert not request.content
                assert dict(request.url.params) == {"offset": "0", "end": "true"}
                state = {"kind": "exited", "code": 0}
                return httpx.Response(200, json={"length": 0, "ended": True})
            state = {"kind": "stopped"}
            return httpx.Response(200)
        connection = Connection("test", "http://dvm.test", transport=httpx.MockTransport(answer))
        spec = {"name": "test", "command": ["cat"] if kind == "program" else []}
        async with AsyncSandbox(connection, {"id": 5, "spec": spec}, input_offset=0) as sandbox:
            if kind == "explicit":
                assert await sandbox.end() == 0
        assert calls == [f"/api/sandboxes/5/{'stop' if kind in ('paused', 'program') else 'input'}"]
    asyncio.run(run())
