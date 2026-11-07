"""``dvm-run login`` and the key it saves, against answers of the test's own."""

import asyncio
import json
import stat

import httpx
import pytest

from dvm import DvmError, NotFoundError
from dvm.connection import Connection
from dvm.credentials import credentials_path, saved_key
from dvm.login import login

URL = "http://dvm.test"


@pytest.fixture(autouse=True)
def config_of_its_own(tmp_path, monkeypatch):
    monkeypatch.setenv("XDG_CONFIG_HOME", str(tmp_path))
    monkeypatch.delenv("DVM_API_KEY", raising=False)
    monkeypatch.delenv("DVM_API_URL", raising=False)


def controller(approved_after, secret="dvm_" + "a" * 64):
    """Answers a login as dvm.run does, approved after the terminal asked
    for its key ``approved_after`` times."""
    asked = []

    def answer(request):
        path = request.url.path
        if path == "/api/cli-logins":
            assert json.loads(request.content)["machine"]
            started = {
                "approve_url": "http://dvm.test/cli-login/page",
                "code": "BCDF-GHJK",
                "collect_token": "collect",
                "expires_at": 0,
            }
            return httpx.Response(201, json=started)
        if path == "/api/cli-logins/key":
            assert json.loads(request.content) == {"collect_token": "collect"}
            asked.append(request)
            if len(asked) <= approved_after:
                return httpx.Response(200, content=b"null")
            key = {"id": 1, "name": "Login on laptop", "ending": secret[-4:]}
            return httpx.Response(200, json={"key": key, "secret": secret})
        if path == "/api/me":
            assert request.headers["authorization"] == f"Bearer {secret}"
            return httpx.Response(200, json={"login": "alice", "admin": False, "limits": None})
        raise AssertionError(path)

    return httpx.MockTransport(answer), asked


def key_sent_to(url, api_key=None):
    """The key the SDK's requests to ``url`` bear."""
    sent = []

    def answer(request):
        sent.append(request.headers["authorization"].removeprefix("Bearer "))
        return httpx.Response(200, json=None)

    async def ask():
        connection = Connection(api_key, url, transport=httpx.MockTransport(answer))
        await connection.json("GET", "/me")
        await connection.close()

    asyncio.run(ask())
    return sent[0]


def test_a_login_saves_its_key_where_the_sdk_finds_it():
    transport, asked = controller(approved_after=2)
    said = []
    path = login(URL + "/", open_browser=False, say=said.append, transport=transport)

    assert len(asked) == 3
    assert said[0] == "Approve this login in your browser: http://dvm.test/cli-login/page"
    assert "BCDF-GHJK" in said[1]
    assert "as alice" in said[2]
    assert path == credentials_path()
    assert stat.S_IMODE(path.stat().st_mode) == 0o600
    assert key_sent_to(URL) == "dvm_" + "a" * 64


def test_each_dvm_run_keeps_its_own_key():
    login(URL, open_browser=False, say=lambda _: None, transport=controller(0)[0])
    other = "http://other.test"
    login(other, open_browser=False, say=lambda _: None, transport=controller(0, "dvm_b")[0])
    assert key_sent_to(URL) == "dvm_" + "a" * 64
    assert key_sent_to(other) == "dvm_b"
    with pytest.raises(DvmError, match="no API key for http://third.test: run `dvm-run login`"):
        Connection(api_url="http://third.test")


def test_a_key_given_or_in_the_environment_comes_before_the_saved_one(monkeypatch):
    login(URL, open_browser=False, say=lambda _: None, transport=controller(0)[0])
    assert key_sent_to(URL, api_key="dvm_given") == "dvm_given"
    monkeypatch.setenv("DVM_API_KEY", "dvm_environment")
    assert key_sent_to(URL) == "dvm_environment"


def test_an_expired_login_says_to_log_in_again():
    message = "no such login: it expired, or was used; run `dvm-run login` again"

    def answer(request):
        if request.url.path == "/api/cli-logins":
            started = {"approve_url": "u", "code": "c", "collect_token": "t", "expires_at": 0}
            return httpx.Response(201, json=started)
        return httpx.Response(404, json={"message": message})

    with pytest.raises(NotFoundError, match="run `dvm-run login` again"):
        login(URL, open_browser=False, say=lambda _: None, transport=httpx.MockTransport(answer))
    assert saved_key(URL) is None


def test_a_credentials_file_it_didnt_write_is_refused():
    credentials_path().parent.mkdir(parents=True)
    credentials_path().write_text("not json")
    with pytest.raises(DvmError, match="delete it, and log in again"):
        saved_key(URL)
