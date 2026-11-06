"""A sandbox: a live VM made from an image, which takes commands and files
until it's stopped, or it fails. Everything it runs is its execution,
which replays once it ends."""

from __future__ import annotations

import asyncio
from typing import Any, Awaitable, Mapping, Optional, TypeVar

from dvm.commands import AsyncCommands
from dvm.connection import Connection
from dvm.errors import ConflictError, SandboxEndedError, SandboxFailedError
from dvm.files import AsyncFiles
from dvm.run_state import RunState

T = TypeVar("T")

# How long a stop waits for its sandbox's log, which the runner reports once
# dvm has ended; a runner that went away meanwhile never does.
_LOG_WAIT_SECONDS = 60.0


class AsyncSandbox:
    """A sandbox, from ``await AsyncSandbox.create(image)``, or another
    process's, from ``await AsyncSandbox.connect(sandbox_id)``. Its ``id`` is
    its execution's too. As a context manager, it's stopped at the end."""

    def __init__(self, connection: Connection, sandbox: Mapping[str, Any]) -> None:
        self._connection = connection
        self.id: int = sandbox["id"]
        self._path = f"/sandboxes/{self.id}"
        self.name: str = sandbox["spec"]["name"]
        self.commands = AsyncCommands(self)
        self.files = AsyncFiles(self)

    @classmethod
    async def create(
        cls,
        image: str,
        *,
        cpus: Optional[int] = None,
        memory_mib: Optional[int] = None,
        seed: Optional[int] = None,
        env: Optional[Mapping[str, str]] = None,
        name: Optional[str] = None,
        api_key: Optional[str] = None,
        api_url: Optional[str] = None,
    ) -> AsyncSandbox:
        """A sandbox of ``image`` (as ``docker run`` takes it), once it's
        running. Left out: 1 vCPU, 1024 MiB, seed 0, no variables beyond the
        image's, and named after the image."""
        draft: dict[str, Any] = {"image": image}
        for field, value in (("cpus", cpus), ("memory_mib", memory_mib), ("seed", seed), ("name", name)):
            if value is not None:
                draft[field] = value
        if env:
            draft["environment"] = [{"name": key, "value": value} for key, value in env.items()]
        connection = Connection(api_key, api_url)
        try:
            sandbox = cls(connection, await connection.json("POST", "/sandboxes", body=draft))
            await sandbox._wait_until_running()
        except BaseException:
            await connection.close()
            raise
        return sandbox

    @classmethod
    async def connect(
        cls,
        sandbox_id: int,
        *,
        api_key: Optional[str] = None,
        api_url: Optional[str] = None,
    ) -> AsyncSandbox:
        """A handle to a sandbox made elsewhere."""
        connection = Connection(api_key, api_url)
        try:
            return cls(connection, await connection.json("GET", f"/sandboxes/{sandbox_id}"))
        except BaseException:
            await connection.close()
            raise

    async def state(self) -> RunState:
        """Where it is: ``queued``, ``running``, ``paused``, ``stopped``, ``failed``."""
        sandbox = await self._connection.json("GET", self._path)
        return RunState.from_api(sandbox["run"]["state"])

    async def pause(self) -> None:
        await self._while_live(self._connection.json("POST", f"{self._path}/pause"))

    async def resume(self) -> None:
        await self._while_live(self._connection.json("POST", f"{self._path}/resume"))

    async def stop(self) -> None:
        """Stops it, which ends its execution, and returns once its log is
        in, so that the execution replays. A sandbox that already ended
        stays as it is."""
        try:
            run = await self._connection.json("POST", f"{self._path}/stop")
        except ConflictError:
            run = (await self._connection.json("GET", self._path))["run"]
            if RunState.from_api(run["state"]).is_active:
                raise
        await self._wait_for_log(run)

    async def close(self) -> None:
        """Closes its connection; the sandbox goes on."""
        await self._connection.close()

    async def __aenter__(self) -> AsyncSandbox:
        return self

    async def __aexit__(self, *_: object) -> None:
        try:
            await self.stop()
        finally:
            await self.close()

    async def _wait_until_running(self) -> None:
        sandbox = await self._connection.json("GET", self._path)
        while True:
            run = sandbox["run"]
            state = RunState.from_api(run["state"])
            if state.kind == "running":
                return
            if not state.is_active:
                raise _ended_error(self.id, state)
            sandbox = await self._connection.json(
                "GET", self._path, params={"newer_than": run["version"]}
            )

    async def _wait_for_log(self, run: Mapping[str, Any]) -> None:
        """A stopped sandbox's runner ends dvm, then reports its log's size
        as the run's next version; one that never reached a runner has none."""
        loop = asyncio.get_running_loop()
        deadline = loop.time() + _LOG_WAIT_SECONDS
        while (
            run["state"]["kind"] == "stopped"
            and run["runner"] is not None
            and run["log_bytes"] is None
            and loop.time() < deadline
        ):
            sandbox = await self._connection.json(
                "GET", self._path, params={"newer_than": run["version"]}
            )
            run = sandbox["run"]

    async def _while_live(self, call: Awaitable[T]) -> T:
        """The call's answer; if it's refused because the sandbox ended,
        that the sandbox ended, and how."""
        try:
            return await call
        except ConflictError as refused:
            ended = await self._ended_error()
            if ended is None:
                raise
            raise ended from refused

    async def _ended_error(self) -> Optional[SandboxEndedError]:
        """What a call to it raises if it has ended; None if it hasn't."""
        state = await self.state()
        return None if state.is_active else _ended_error(self.id, state)


def _ended_error(sandbox_id: int, state: RunState) -> SandboxEndedError:
    if state.kind == "failed":
        return SandboxFailedError(sandbox_id, state)
    return SandboxEndedError(sandbox_id, state)
