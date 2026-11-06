"""The connection to dvm.run's API: its address, the key every request
bears, what a refusal means, and server-sent events."""

from __future__ import annotations

import json
import os
from typing import Any, AsyncIterator, Mapping, Optional

import httpx

from dvm.errors import (
    ApiError,
    AuthenticationError,
    ConflictError,
    DvmError,
    InvalidRequestError,
    NotFoundError,
)

DEFAULT_API_URL = "https://sandbox.dvm.run"

# The controller answers a `?newer_than=` read within 30 s, and keeps a quiet
# event stream alive with comments more often than that.
_READ_TIMEOUT_SECONDS = 60.0

_REFUSALS: Mapping[int, type[ApiError]] = {
    400: InvalidRequestError,
    401: AuthenticationError,
    404: NotFoundError,
    409: ConflictError,
}


class Connection:
    """dvm.run's API, as the user whose key it bears: ``api_key``, else
    ``DVM_API_KEY``; at ``api_url``, else ``DVM_API_URL``, else dvm.run's."""

    def __init__(
        self,
        api_key: Optional[str] = None,
        api_url: Optional[str] = None,
        *,
        transport: Optional[httpx.AsyncBaseTransport] = None,
    ) -> None:
        key = api_key or os.environ.get("DVM_API_KEY")
        if not key:
            raise DvmError("no API key: pass api_key=, or set DVM_API_KEY")
        url = api_url or os.environ.get("DVM_API_URL") or DEFAULT_API_URL
        self._client = httpx.AsyncClient(
            base_url=url.rstrip("/") + "/api",
            headers={"Authorization": f"Bearer {key}"},
            timeout=httpx.Timeout(30.0, read=_READ_TIMEOUT_SECONDS),
            transport=transport,
        )

    async def json(
        self,
        method: str,
        path: str,
        *,
        params: Optional[Mapping[str, Any]] = None,
        body: Any = None,
    ) -> Any:
        """The answer's JSON; None for an empty one."""
        response = await self._send(method, path, params=params, json=body)
        return response.json() if response.content else None

    async def bytes(
        self,
        method: str,
        path: str,
        *,
        params: Optional[Mapping[str, Any]] = None,
        content: Optional[bytes] = None,
    ) -> bytes:
        """The answer's body, as it is."""
        response = await self._send(method, path, params=params, content=content)
        return response.content

    async def events(
        self, path: str, *, params: Optional[Mapping[str, Any]] = None
    ) -> AsyncIterator[Any]:
        """Each server-sent event's data, as JSON, until the stream ends."""
        async with self._client.stream("GET", path, params=params) as response:
            if response.is_error:
                await response.aread()
                raise _refusal(response)
            data: list[str] = []
            async for line in response.aiter_lines():
                if line.startswith("data:"):
                    data.append(line[5:].removeprefix(" "))
                elif line == "" and data:
                    yield json.loads("\n".join(data))
                    data = []

    async def close(self) -> None:
        await self._client.aclose()

    async def _send(self, method: str, path: str, **request: Any) -> httpx.Response:
        response = await self._client.request(method, path, **request)
        if response.is_error:
            raise _refusal(response)
        return response


def _refusal(response: httpx.Response) -> ApiError:
    """The error a refusal is: its status's, with the controller's reason."""
    try:
        message = response.json()["message"]
    except (ValueError, KeyError, TypeError):
        message = response.text or response.reason_phrase
    return _REFUSALS.get(response.status_code, ApiError)(response.status_code, message)
