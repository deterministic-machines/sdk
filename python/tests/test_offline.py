"""The SDK against answers of the test's own: what needs no platform."""

import asyncio
import base64
import json

import httpx
import pytest

from dvm import AsyncCommandHandle, AsyncSandbox, DvmError, SandboxFailedError
from dvm.connection import Connection
from dvm.execution import _moment_of


def sandbox_answered_by(answer):
    transport = httpx.MockTransport(answer)
    connection = Connection("dvm_test", "http://dvm.test", transport=transport)
    return AsyncSandbox(connection, {"id": 5, "spec": {"name": "test"}})


def events(*sent):
    body = "".join(f": kept alive\n\ndata: {json.dumps(event)}\n\n" for event in sent)
    return httpx.Response(200, headers={"content-type": "text/event-stream"}, content=body)


def output(stream, offset, chunk):
    encoded = base64.b64encode(chunk).decode()
    return {"kind": "output", "stream": stream, "offset": offset, "chunk": encoded}


def command(state):
    return {
        "kind": "command",
        "command": {
            "number": 1,
            "command": "printf",
            "argv": None,
            "state": state,
            "started_at": None,
            "ended_at": None,
        },
    }


def test_a_dropped_output_stream_is_followed_again_from_where_it_got_to():
    asked = []
    e_acute = "é".encode()

    def answer(request):
        assert request.url.path == "/api/sandboxes/5/commands/1/output"
        asked.append((request.url.params["stdout_offset"], request.url.params["stderr_offset"]))
        if len(asked) == 1:
            return events(output("stdout", 0, b"hel"), output("stderr", 0, e_acute[:1]))
        return events(
            output("stdout", 3, b"lo"),
            output("stderr", 1, e_acute[1:]),
            command({"kind": "exited", "code": 0}),
        )

    stderr = []
    handle = AsyncCommandHandle(sandbox_answered_by(answer), 1)
    result = asyncio.run(handle.wait(on_stderr=stderr.append))
    assert (result.stdout, result.stderr, stderr) == ("hello", "é", ["é"])
    assert asked == [("0", "0"), ("3", "1")]


def test_a_stream_that_never_ends_its_command_is_an_error():
    def answer(request):
        return events(command({"kind": "running"}))

    handle = AsyncCommandHandle(sandbox_answered_by(answer), 1)
    with pytest.raises(DvmError, match="ended before its command"):
        asyncio.run(handle.wait())


def test_a_call_refused_as_its_sandbox_failed_says_why():
    def answer(request):
        if request.method == "POST":
            return httpx.Response(409, json={"message": "sandbox 5 is failed"})
        failed = {"kind": "failed", "reason": "syscall 999 isn't served"}
        return httpx.Response(200, json={"id": 5, "run": {"state": failed}})

    with pytest.raises(SandboxFailedError, match="syscall 999"):
        asyncio.run(sandbox_answered_by(answer).commands.run("true"))


def test_a_moment_is_read_as_dvm_writes_it():
    assert _moment_of("3:17") == {"kind": "event", "thread": 3, "operation": 17}
    assert _moment_of("@12.5") == {"kind": "time", "at_us": 12500}
    assert _moment_of("@7") == {"kind": "time", "at_us": 7000}


def test_a_key_is_needed(monkeypatch):
    monkeypatch.delenv("DVM_API_KEY", raising=False)
    with pytest.raises(DvmError, match="DVM_API_KEY"):
        Connection()
