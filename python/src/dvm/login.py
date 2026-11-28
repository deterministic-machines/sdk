"""Logging in from a terminal (``dvm-run login``): dvm.run makes an API key
for the user who approves the login in their browser, and the terminal
saves it where the SDK finds it, so no one copies a key. The page that
approves it opens here; over SSH, the user opens the link it prints
wherever they like."""

from __future__ import annotations

import asyncio
import os
import socket
import webbrowser
from collections.abc import Awaitable, Callable
from pathlib import Path
from typing import Optional

import httpx

from dvm.blocking import _run
from dvm.connection import Connection, DEFAULT_API_URL
from dvm.credentials import save_key


class _SyncTransport(httpx.AsyncBaseTransport):
    def __init__(self, transport: httpx.BaseTransport) -> None:
        self._transport = transport

    async def handle_async_request(self, request: httpx.Request) -> httpx.Response:
        return self._transport.handle_request(request)


def login(
    api_url: Optional[str] = None,
    *,
    open_browser: bool = True,
    say: Callable[[str], None] = lambda line: print(line, flush=True),
    transport: Optional[httpx.BaseTransport | httpx.AsyncBaseTransport] = None,
    sleep: Callable[[float], Awaitable[None]] = asyncio.sleep,
) -> Path:
    """Logs this machine in to the dvm.run at ``api_url``, else
    ``DVM_API_URL``, else dvm.run's, once its user approves the login in
    their browser; the file its key is saved in. Says what to do, and what
    it did, with ``say``."""
    url = (api_url or os.environ.get("DVM_API_URL") or DEFAULT_API_URL).rstrip("/")
    async_transport = (
        transport
        if transport is None or isinstance(transport, httpx.AsyncBaseTransport)
        else _SyncTransport(transport)
    )
    return _run(_login(url, open_browser=open_browser, say=say, transport=async_transport, sleep=sleep))


async def _login(
    url: str,
    *,
    open_browser: bool,
    say: Callable[[str], None],
    transport: Optional[httpx.AsyncBaseTransport],
    sleep: Callable[[float], Awaitable[None]],
) -> Path:
    terminal = Connection(api_url=url, require_key=False, transport=transport, sleep=sleep)
    try:
        started = await terminal.json(
            "POST", "/cli-logins", body={"machine": socket.gethostname()},
        )
        say(f"Approve this login in your browser: {started['approve_url']}")
        say(f"The page shows the code {started['code']}.")
        if open_browser:
            webbrowser.open(started["approve_url"])
        collect = {"collect_token": started["collect_token"]}
        created = None
        while created is None:
            created = await terminal.json("POST", "/cli-logins/key", body=collect)
    finally:
        await terminal.close()
    key = created["secret"]
    user = Connection(api_key=key, api_url=url, transport=transport, sleep=sleep)
    try:
        me = await user.json("GET", "/me")
    finally:
        await user.close()
    path = save_key(url, key)
    say(f"Logged in to {url} as {me['login']}, with the key {created['key']['name']}.")
    say(f"It's saved in {path}.")
    return path
