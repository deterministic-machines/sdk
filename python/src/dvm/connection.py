"""The connection to dvm.run's API: its address, the key every request
bears, and what a refusal means."""

from __future__ import annotations

import os
from typing import Any, Mapping, Optional

import httpx

from dvm.credentials import saved_key
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
    """dvm.run's API, at ``api_url``, else ``DVM_API_URL``, else dvm.run's;
    as the user whose key it bears: ``api_key``, else ``DVM_API_KEY``, else
    the one ``dvm-run login`` saved for that dvm.run."""

    def __init__(
        self,
        api_key: Optional[str] = None,
        api_url: Optional[str] = None,
        *,
        transport: Optional[httpx.AsyncBaseTransport] = None,
    ) -> None:
        url = (api_url or os.environ.get("DVM_API_URL") or DEFAULT_API_URL).rstrip("/")
        key = api_key or os.environ.get("DVM_API_KEY") or saved_key(url)
        if not key:
            raise DvmError(
                f"no API key for {url}: run `dvm-run login`, pass api_key=, or set DVM_API_KEY"
            )
        self._client = httpx.AsyncClient(
            base_url=url + "/api",
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

    async def close(self) -> None:
        await self._client.aclose()

    async def _send(self, method: str, path: str, **request: Any) -> httpx.Response:
        response = await self._client.request(method, path, **request)
        if response.is_error:
            raise refusal_of(response)
        return response


def refusal_of(response: httpx.Response) -> ApiError:
    """The error a refusal is: its status's, with the controller's reason."""
    try:
        message = response.json()["message"]
    except (ValueError, KeyError, TypeError):
        message = response.text or response.reason_phrase
    return _REFUSALS.get(response.status_code, ApiError)(response.status_code, message)
