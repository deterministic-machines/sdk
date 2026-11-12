"""A sandbox: one recorded run of one program, the image's own or the one
given as it's made. Its execution replays once the program exited."""

from __future__ import annotations

from typing import Any, Awaitable, Mapping, Optional, TypeVar

from dvm.connection import Connection
from dvm.errors import ConflictError, SandboxEndedError, SandboxFailedError
from dvm.execution import Output
from dvm.run_state import RunState

T = TypeVar("T")

class AsyncSandbox:
    """A sandbox, from ``await AsyncSandbox.create(image)``, or another
    process's, from ``await AsyncSandbox.connect(sandbox_id)``. Its ``id`` is
    its execution's too. As a context manager, it's stopped at the end."""

    def __init__(self, connection: Connection, sandbox: Mapping[str, Any]) -> None:
        self._connection = connection
        self.id: int = sandbox["id"]
        self._path = f"/sandboxes/{self.id}"
        self.name: str = sandbox["spec"]["name"]

    @classmethod
    async def create(
        cls,
        image: str,
        *,
        command: Optional[list[str]] = None,
        entrypoint: Optional[str] = None,
        cpus: Optional[int] = None,
        memory_mib: Optional[int] = None,
        seed: Optional[int] = None,
        env: Optional[Mapping[str, str]] = None,
        name: Optional[str] = None,
        api_key: Optional[str] = None,
        api_url: Optional[str] = None,
    ) -> AsyncSandbox:
        """A sandbox of ``image`` (as ``docker run`` takes it), once its
        program started. Left out: the image's own command and entrypoint,
        1 vCPU, 1024 MiB, seed 0, no variables beyond the image's, and named
        after the image."""
        draft: dict[str, Any] = {"image": image}
        given = {
            "command": command,
            "entrypoint": entrypoint,
            "cpus": cpus,
            "memory_mib": memory_mib,
            "seed": seed,
            "name": name,
        }
        draft.update({field: value for field, value in given.items() if value is not None})
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
        """Where it is: ``queued``, ``running``, ``paused``, ``exited``, ``stopped``, ``failed``."""
        sandbox = await self._connection.json("GET", self._path)
        return RunState.from_api(sandbox["run"]["state"])

    async def pause(self) -> None:
        await self._while_live(self._connection.json("POST", f"{self._path}/pause"))

    async def resume(self) -> None:
        await self._while_live(self._connection.json("POST", f"{self._path}/resume"))

    async def stop(self) -> None:
        """Stops it. A stopped sandbox's log is cut short, and doesn't
        replay. One that already ended stays as it is."""
        try:
            await self._connection.json("POST", f"{self._path}/stop")
        except ConflictError:
            if (await self.state()).is_active:
                raise

    async def wait(self) -> int:
        """The program's exit code; stopped and failed runs raise their error."""
        sandbox = await self._connection.json("GET", self._path)
        while True:
            run = sandbox["run"]
            state = RunState.from_api(run["state"])
            if state.kind == "exited":
                return state.exit_code
            if not state.is_active:
                raise _ended_error(self.id, state)
            sandbox = await self._connection.json(
                "GET", self._path, params={"newer_than": run["version"]}
            )

    async def output(self) -> Output:
        """What the program wrote so far (``Output``)."""
        return Output.from_api(await self._connection.json("GET", f"/executions/{self.id}/output"))

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
            if state.kind in ("running", "paused", "exited"):
                return
            if not state.is_active:
                raise _ended_error(self.id, state)
            sandbox = await self._connection.json(
                "GET", self._path, params={"newer_than": run["version"]}
            )

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
