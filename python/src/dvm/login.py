"""Logging in from a terminal (``dvm-run login``): dvm.run makes an API key
for the user who approves the login in their browser, and the terminal
saves it where the SDK finds it, so no one copies a key. The page that
approves it opens here; over SSH, the user opens the link it prints
wherever they like."""

from __future__ import annotations

import os
import socket
import webbrowser
from pathlib import Path
from typing import Any, Callable, Optional

import httpx

from dvm.connection import DEFAULT_API_URL, refusal_of
from dvm.credentials import save_key

# dvm.run answers a terminal waiting for its key within 30 s.
_READ_TIMEOUT_SECONDS = 60.0


def login(
    api_url: Optional[str] = None,
    *,
    open_browser: bool = True,
    say: Callable[[str], None] = lambda line: print(line, flush=True),
    transport: Optional[httpx.BaseTransport] = None,
) -> Path:
    """Logs this machine in to the dvm.run at ``api_url``, else
    ``DVM_API_URL``, else dvm.run's, once its user approves the login in
    their browser; the file its key is saved in. Says what to do, and what
    it did, with ``say``."""
    url = (api_url or os.environ.get("DVM_API_URL") or DEFAULT_API_URL).rstrip("/")
    with httpx.Client(
        base_url=url + "/api",
        timeout=httpx.Timeout(30.0, read=_READ_TIMEOUT_SECONDS),
        transport=transport,
    ) as client:
        started = _answer(client.post("/cli-logins", json={"machine": socket.gethostname()}))
        say(f"Approve this login in your browser: {started['approve_url']}")
        say(f"The page shows the code {started['code']}.")
        if open_browser:
            webbrowser.open(started["approve_url"])
        collect = {"collect_token": started["collect_token"]}
        created = None
        while created is None:
            created = _answer(client.post("/cli-logins/key", json=collect))
        key = created["secret"]
        me = _answer(client.get("/me", headers={"Authorization": f"Bearer {key}"}))
    path = save_key(url, key)
    say(f"Logged in to {url} as {me['login']}, with the key {created['key']['name']}.")
    say(f"It's saved in {path}.")
    return path


def _answer(response: httpx.Response) -> Any:
    if response.is_error:
        raise refusal_of(response)
    return response.json()
