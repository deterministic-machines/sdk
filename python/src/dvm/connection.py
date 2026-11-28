"""The connection to dvm.run's API: its address, the key every request
bears, and what a refusal means."""

from __future__ import annotations

import asyncio
import os
from collections.abc import Awaitable, Callable, Mapping
from typing import Any, Optional

import httpx

from dvm.credentials import saved_key
from dvm.errors import (
    ApiError,
    AuthenticationError,
    ConflictError,
    DvmError,
    InvalidRequestError,
    NetworkError,
    NotFoundError,
)

DEFAULT_API_URL = "https://sandbox.dvm.run"

# The controller answers a `?newer_than=` read within 30 s, and keeps a quiet
# event stream alive with comments more often than that.
_READ_TIMEOUT_SECONDS = 60.0
_RETRY_DELAYS = (0.1, 0.25, 0.5, 1.0)
_MAX_RETRY_AFTER_SECONDS = 5.0

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
        require_key: bool = True,
        transport: Optional[httpx.AsyncBaseTransport] = None,
        sleep: Callable[[float], Awaitable[None]] = asyncio.sleep,
    ) -> None:
        self._url = (api_url or os.environ.get("DVM_API_URL") or DEFAULT_API_URL).rstrip("/")
        key = api_key or (
            os.environ.get("DVM_API_KEY") or saved_key(self._url)
            if require_key
            else None
        )
        if require_key and not key:
            raise DvmError(
                f"no API key for {self._url}: run `dvm-run login`, pass api_key=, or set DVM_API_KEY"
            )
        headers = {"Authorization": f"Bearer {key}"} if key else {}
        self._sleep = sleep
        self._client = httpx.AsyncClient(
            base_url=self._url + "/api",
            headers=headers,
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
        idempotent: Optional[bool] = None,
    ) -> Any:
        """The answer's JSON; None for an empty one."""
        safe = method in ("GET", "PUT", "DELETE") if idempotent is None else idempotent
        response = await self._send(
            method, path, idempotent=safe, params=params, json=body,
        )
        return response.json() if response.content else None

    async def input(self, path: str, data: bytes, *, offset: int, end: bool) -> Any:
        response, attempt = await self._request(
            "POST", path, idempotent=True,
            params={"offset": offset, "end": str(end).lower()},
            content=data, headers={"Content-Type": "application/octet-stream"},
        )
        if response.is_error:
            refusal = refusal_of(response)
            if (
                attempt > 0
                and end
                and not data
                and isinstance(refusal, ConflictError)
                and refusal.message == f"input already ended at length {offset}"
            ):
                return {"length": offset, "ended": True}
            raise refusal
        return response.json()

    async def bytes(self, path: str, *, params: Optional[Mapping[str, Any]] = None) -> bytes:
        return (await self._send("GET", path, idempotent=True, params=params)).content

    async def close(self) -> None:
        await self._client.aclose()

    async def _send(
        self, method: str, path: str, *, idempotent: bool, **request: Any
    ) -> httpx.Response:
        response, _ = await self._request(method, path, idempotent=idempotent, **request)
        if response.is_error:
            raise refusal_of(response)
        return response

    async def _request(
        self, method: str, path: str, *, idempotent: bool, **request: Any
    ) -> tuple[httpx.Response, int]:
        for attempt, delay in enumerate((*_RETRY_DELAYS, None)):
            try:
                response = await self._client.request(method, path, **request)
            except httpx.HTTPError as error:
                if delay is None or not _retryable_error(error, idempotent=idempotent):
                    raise NetworkError(
                        f"couldn't reach {self._url}/api{path}: {error}"
                    ) from error
                await self._sleep(delay)
                continue
            if delay is not None and _retryable_response(response, idempotent=idempotent):
                await self._sleep(_retry_delay(response, delay))
                continue
            return response, attempt
        raise AssertionError("unreachable")


def _retryable_error(error: httpx.HTTPError, *, idempotent: bool) -> bool:
    if isinstance(error, (httpx.ConnectError, httpx.ConnectTimeout, httpx.PoolTimeout)):
        return True
    return idempotent and isinstance(
        error,
        (httpx.NetworkError, httpx.RemoteProtocolError, httpx.TimeoutException, httpx.ProxyError),
    )


def _retryable_response(response: httpx.Response, *, idempotent: bool) -> bool:
    status = response.status_code
    if status in (408, 429):
        return True
    if not idempotent:
        return False
    if status in (502, 504):
        return True
    return status == 503 and not _is_controller_refusal(response)


def _is_controller_refusal(response: httpx.Response) -> bool:
    try:
        body = response.json()
    except ValueError:
        return False
    return isinstance(body, Mapping) and isinstance(body.get("error"), str)


def _retry_delay(response: httpx.Response, default: float) -> float:
    header = response.headers.get("retry-after", "").strip()
    if header.isdigit():
        return min(_MAX_RETRY_AFTER_SECONDS, max(default, float(int(header))))
    return default


def refusal_of(response: httpx.Response) -> ApiError:
    """The error a refusal is: its status's, with the controller's reason."""
    try:
        message = response.json()["message"]
    except (ValueError, KeyError, TypeError):
        message = response.text or response.reason_phrase
    return _REFUSALS.get(response.status_code, ApiError)(response.status_code, message)
