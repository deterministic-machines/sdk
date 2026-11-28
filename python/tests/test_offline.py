"""Lifecycle and byte output against fixed HTTP answers."""
import asyncio
import json

import httpx
import pytest

from dvm import (
    ApiError,
    AsyncSandbox,
    DvmError,
    NetworkError,
    SandboxEndedError,
    SandboxFailedError,
)
from dvm.connection import Connection
from dvm.frame import encode_frame
from dvm.login import login


def sandbox_answered_by(answer):
    connection = Connection("dvm_test", "http://dvm.test", transport=httpx.MockTransport(answer))
    return AsyncSandbox(connection, {"id": 5, "spec": {"name": "test"}})


def test_wait_uses_versions_and_returns_a_nonzero_exit():
    states = iter([{"kind": "running"}, {"kind": "exited", "code": 3}])
    def answer(request):
        if request.url.params:
            assert request.url.params["newer_than"] == "1"
        return httpx.Response(200, json={"run": {"state": next(states), "version": 1}})
    assert asyncio.run(sandbox_answered_by(answer).wait()) == 3


@pytest.mark.parametrize("state,error", [
    ({"kind": "stopped"}, SandboxEndedError),
    ({"kind": "failed", "reason": "unserved syscall"}, SandboxFailedError),
])
def test_wait_reports_stopped_and_failed(state, error):
    sandbox = sandbox_answered_by(lambda request: httpx.Response(200, json={"run": {"state": state}}))
    with pytest.raises(error):
        asyncio.run(sandbox.wait())


def test_output_keeps_every_byte():
    data = bytes(range(256))
    def answer(request):
        assert request.url.path == "/api/sandboxes/5/output"
        return httpx.Response(200, content=encode_frame({"stdout": len(data), "stderr": 1, "more": False, "cut": False}, data + b"\0"))
    output = asyncio.run(sandbox_answered_by(answer).output())
    assert (output.stdout, output.stderr) == (data, b"\0")


def test_create_accepts_an_already_exited_program(monkeypatch):
    def answer(request):
        return httpx.Response(200, json={"id": 5, "spec": {"name": "test"}, "run": {"state": {"kind": "exited", "code": 0}}})
    monkeypatch.setattr("dvm.sandbox.Connection", lambda *args: Connection("test", "http://dvm.test", transport=httpx.MockTransport(answer)))
    async def create():
        sandbox = await AsyncSandbox.create("alpine", command=["true"])
        await sandbox.close()
    asyncio.run(create())


def test_a_key_is_needed(monkeypatch):
    monkeypatch.delenv("DVM_API_KEY", raising=False)
    with pytest.raises(DvmError, match="DVM_API_KEY"):
        Connection(api_url="http://missing.test")


def test_set_network_puts_the_mode_and_an_allowlists_entries():
    bodies = []
    def answer(request):
        assert (request.method, request.url.path) == ("PUT", "/api/sandboxes/5/network")
        bodies.append(json.loads(request.content))
        return httpx.Response(200, json={"id": 5, "spec": {"name": "test"}})
    sandbox = sandbox_answered_by(answer)
    asyncio.run(sandbox.set_network("none"))
    asyncio.run(sandbox.set_network("allowlist", ["pypi.org", "8.8.8.0/24"]))
    assert bodies == [{"mode": "none"}, {"mode": "allowlist", "allow": ["pypi.org", "8.8.8.0/24"]}]
    with pytest.raises(ValueError):
        asyncio.run(sandbox.set_network("public", ["pypi.org"]))


def test_output_follows_pages_and_advances_both_offsets():
    data = bytes(range(256)) * (4 * 1024 * 1024 // 256)
    pages = iter([(data, data, True), (b"tail", b"\xfferr", False)])
    offsets = None
    def answer(request):
        # A sandbox still going: `more` stays true, and its output so far
        # ends where a page isn't cut.
        nonlocal offsets
        params = request.url.params
        assert (offsets is None and not params) or (
            (int(params["offset"]), int(params["stderr_offset"])) == offsets)
        stdout, stderr, cut = next(pages)
        offsets = ((offsets or (0, 0))[0] + len(stdout), (offsets or (0, 0))[1] + len(stderr))
        return httpx.Response(200, content=encode_frame(
            {"stdout": len(stdout), "stderr": len(stderr), "more": True, "cut": cut}, stdout + stderr,
        ))
    output = asyncio.run(sandbox_answered_by(answer).output())
    assert output.stdout == data + b"tail"
    assert output.stderr == data + b"\xfferr"


def test_seek_step_and_files():
    seeks = []
    step_calls = 0

    def stop(thread, event, ns, name):
        return {
            "thread": thread,
            "process": 1,
            "operations": event,
            "event": event,
            "recording_ns": ns,
            "at": {
                "syscall": {
                    "number": 59,
                    "name": name,
                    "args": ["0x1", "0x2", "0x0", "0x0", "0x0", "0x0"],
                    "decoded": [{"arg": 0, "path": "/bin/sh"}],
                }
            },
            "stats": {"syscalls": event},
        }

    def answer(request):
        nonlocal step_calls
        path = request.url.path
        if path == "/api/sandboxes/5/points/seek":
            body = json.loads(request.content)
            seeks.append(body)
            target = body["target"]
            if target["kind"] == "at_ns":
                return httpx.Response(200, json=stop(7, 10, target["ns"], "openat"))
            if target["kind"] == "event":
                return httpx.Response(200, json=stop(target["thread"], target["event"], 500, "openat"))
            return httpx.Response(200, json=stop(7, 99, 999, "exit_group"))
        if path == "/api/sandboxes/5/points/step":
            step_calls += 1
            body = json.loads(request.content)
            if step_calls == 1:
                assert (body["thread"], body["event"]) == (7, 10)
                return httpx.Response(200, json=stop(7, 20, 900, "read"))
            assert (body["thread"], body["event"]) == (7, 20)
            return httpx.Response(200, json=None)
        if path == "/api/sandboxes/5/fs/stat":
            assert dict(request.url.params) == {
                "thread": "7",
                "event": "10",
                "path": "/etc/hosts",
                "follow_symlinks": "false",
            }
            return httpx.Response(200, json={"type": "regular", "mode": 420, "uid": 0, "gid": 0, "size": 5, "links": 1, "mtime": [0, 0], "ctime": [0, 0]})
        if path == "/api/sandboxes/5/fs/list":
            assert dict(request.url.params) == {"thread": "7", "event": "10", "path": "/etc"}
            return httpx.Response(200, json=[{"name": "hosts", "raw_name": "686f737473", "type": "regular"}])
        if path == "/api/sandboxes/5/fs/read":
            assert dict(request.url.params) == {"thread": "7", "event": "10", "path": "/etc/hosts"}
            return httpx.Response(200, content=b"hello")
        raise AssertionError(f"unexpected path {path}")

    async def run():
        sandbox = sandbox_answered_by(answer)
        p1 = await sandbox.seek(at_ns=500, watch=["file-syscalls", "syscall:openat"])
        assert (p1.thread, p1.event, p1.ns, p1.at.name) == (7, 10, 500, "openat")
        assert p1.at.decoded[0].value == "/bin/sh"
        p2 = await p1.step()
        assert p2 is not None and p2.event == 20
        assert await p2.step() is None
        stat = await p1.stat("/etc/hosts", follow_symlinks=False)
        assert stat.size == 5
        entries = await p1.list_dir("/etc")
        assert entries[0].raw_name == b"hosts"
        assert await p1.read_file("/etc/hosts") == b"hello"
        with pytest.raises(ValueError):
            await sandbox.seek(at_ns=1, thread=1, event=1)
        with pytest.raises(ValueError):
            await sandbox.seek(thread=1)

    asyncio.run(run())
    assert [entry["target"] for entry in seeks] == [{"kind": "at_ns", "ns": 500}]


def test_idempotent_calls_retry_transient_failures_and_preserve_offsets():
    delays = []

    async def record_sleep(seconds):
        delays.append(seconds)

    attempts = {"get": 0, "input": 0, "end": 0, "seek": 0}

    def answer(request):
        path = request.url.path
        if path == "/api/sandboxes/5" and request.method == "GET":
            attempts["get"] += 1
            if attempts["get"] == 1:
                raise httpx.ConnectError("tls reset", request=request)
            if attempts["get"] == 2:
                return httpx.Response(502, text="Bad Gateway")
            if attempts["get"] == 3:
                return httpx.Response(503, text="Service Unavailable", headers={"Retry-After": "2"})
            return httpx.Response(200, json={"run": {"state": {"kind": "exited", "code": 0}}})
        if path == "/api/sandboxes/5/input":
            if request.url.params["end"] == "true":
                attempts["end"] += 1
                if attempts["end"] == 1:
                    raise httpx.ReadError("dropped response", request=request)
                return httpx.Response(
                    409, json={"message": f"input already ended at length {request.url.params['offset']}"}
                )
            attempts["input"] += 1
            assert request.url.params["offset"] == "0"
            if attempts["input"] == 1:
                raise httpx.RemoteProtocolError("server disconnected", request=request)
            return httpx.Response(200, json={"length": len(request.content), "ended": False})
        if path == "/api/sandboxes/5/points/seek":
            attempts["seek"] += 1
            if attempts["seek"] == 1:
                return httpx.Response(504, text="Gateway Timeout")
            stop = {
                "thread": 1, "process": 1, "operations": 1, "event": 1, "recording_ns": 0,
                "at": {"syscall": {"number": 0, "name": "read", "args": []}}, "stats": {},
            }
            return httpx.Response(200, json=stop)
        raise AssertionError(path)

    async def run():
        connection = Connection(
            "dvm_test", "http://dvm.test",
            transport=httpx.MockTransport(answer), sleep=record_sleep,
        )
        sandbox = AsyncSandbox(connection, {"id": 5, "spec": {"name": "test"}}, input_offset=0)
        assert await sandbox.wait() == 0
        state = await connection.input("/sandboxes/5/input", b"abc", offset=0, end=False)
        assert state == {"length": 3, "ended": False}
        ended = await connection.input("/sandboxes/5/input", b"", offset=3, end=True)
        assert ended == {"length": 3, "ended": True}
        point = await sandbox.seek()
        assert point.event == 1
        await sandbox.close()

    asyncio.run(run())
    assert attempts == {"get": 4, "input": 2, "end": 2, "seek": 2}
    assert delays == [0.1, 0.25, 2.0, 0.1, 0.1, 0.1]


def test_non_idempotent_posts_retry_only_before_execution_and_refusals_do_not_retry(monkeypatch, tmp_path):
    monkeypatch.setenv("XDG_CONFIG_HOME", str(tmp_path))
    delays = []

    async def record_sleep(seconds):
        delays.append(seconds)

    calls = {"pause_connect": 0, "pause_read": 0, "pause_502": 0, "returning": 0, "exhaust": 0}

    def answer(request):
        path = request.url.path
        if path == "/api/sandboxes/1/pause":
            calls["pause_connect"] += 1
            if calls["pause_connect"] == 1:
                raise httpx.ConnectError("handshake failed", request=request)
            if calls["pause_connect"] == 2:
                return httpx.Response(429, text="rate limited", headers={"Retry-After": "99"})
            return httpx.Response(200, json={"state": {"kind": "paused"}})
        if path == "/api/sandboxes/2/pause":
            calls["pause_read"] += 1
            raise httpx.ReadError("reset after send", request=request)
        if path == "/api/sandboxes/3/pause":
            calls["pause_502"] += 1
            return httpx.Response(502, text="Bad Gateway")
        if path == "/api/sandboxes/4/fs/stat":
            calls["returning"] += 1
            return httpx.Response(503, json={"error": "returning", "message": "still seeking"})
        if path == "/api/sandboxes/5":
            calls["exhaust"] += 1
            raise httpx.ConnectTimeout("timed out", request=request)
        raise AssertionError(path)

    async def run():
        connection = Connection(
            "dvm_test", "http://dvm.test",
            transport=httpx.MockTransport(answer), sleep=record_sleep,
        )
        await AsyncSandbox(connection, {"id": 1, "spec": {"name": "a"}}).pause()
        with pytest.raises(NetworkError):
            await AsyncSandbox(connection, {"id": 2, "spec": {"name": "b"}}).pause()
        with pytest.raises(ApiError) as gateway:
            await AsyncSandbox(connection, {"id": 3, "spec": {"name": "c"}}).pause()
        assert gateway.value.status == 502
        with pytest.raises(ApiError, match="still seeking") as returning:
            await connection.json("GET", "/sandboxes/4/fs/stat")
        assert returning.value.status == 503
        with pytest.raises(NetworkError):
            await AsyncSandbox(connection, {"id": 5, "spec": {"name": "e"}}).state()
        await connection.close()

    asyncio.run(run())
    assert calls == {"pause_connect": 3, "pause_read": 1, "pause_502": 1, "returning": 1, "exhaust": 5}
    assert delays == [0.1, 5.0, 0.1, 0.25, 0.5, 1.0]

    login_calls = 0

    def login_answer(request):
        nonlocal login_calls
        login_calls += 1
        if login_calls == 1:
            raise httpx.ConnectError("offline", request=request)
        if request.url.path == "/api/cli-logins":
            return httpx.Response(201, json={
                "approve_url": "http://dvm.test/cli-login/p", "code": "BCDF-GHJK",
                "collect_token": "tok", "expires_at": 0,
            })
        if request.url.path == "/api/cli-logins/key":
            return httpx.Response(200, json={"key": {"id": 1, "name": "k", "ending": "aaaa"}, "secret": "dvm_" + "a" * 64})
        return httpx.Response(200, json={"login": "alice", "admin": False, "limits": None})

    login("http://dvm.test", open_browser=False, say=lambda _: None,
          transport=httpx.MockTransport(login_answer), sleep=record_sleep)
    assert login_calls == 4
