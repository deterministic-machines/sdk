"""Lifecycle and byte output against fixed HTTP answers."""
import asyncio
import base64
import json

import httpx
import pytest

from dvm import AsyncSandbox, DvmError, SandboxEndedError, SandboxFailedError, Verdict
from dvm.connection import Connection


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
        assert request.url.path == "/api/executions/5/output"
        return httpx.Response(200, json={"stdout": base64.b64encode(data).decode(), "stderr": "AA=="})
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


def test_verdict_has_only_program_details():
    verdict = Verdict.from_api({"kind": "other_output", "stream": "stderr", "at": 7})
    assert (verdict.stream, verdict.at, verdict.same) == ("stderr", 7, False)


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
