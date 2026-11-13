"""A recorded sandbox: a session that takes commands, or one run of a
supplied program. Its execution replays once its process exited."""

from __future__ import annotations

from typing import Any, Awaitable, Mapping, Optional, TypeVar

from dvm.connection import Connection
from dvm.errors import ConflictError, DvmError, SandboxEndedError, SandboxFailedError
from dvm.execution import Output
from dvm.run_state import RunState
from dvm.sandbox_session import AsyncCommand, CommandResult, SandboxSession

T = TypeVar("T")

class AsyncSandbox:
    """A sandbox, from ``await AsyncSandbox.create(image)``, or another
    process's, from ``await AsyncSandbox.connect(sandbox_id)``. Its ``id`` is
    its execution's too. With no command or entrypoint it takes commands;
    its context ends input and waits for exit. A paused sandbox, or one
    running a supplied program, is stopped at the end instead."""

    def __init__(
        self, connection: Connection, sandbox: Mapping[str, Any],
        *, input_offset: Optional[int] = None,
    ) -> None:
        self._connection = connection
        self.id: int = sandbox["id"]
        self._path = f"/sandboxes/{self.id}"
        self.name: str = sandbox["spec"]["name"]
        self._takes_commands = (
            not sandbox["spec"].get("command") and sandbox["spec"].get("entrypoint") is None
        )
        self._session: Optional[SandboxSession] = None
        self._input_offset = input_offset

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
        process started. Without command or entrypoint it runs the sandbox
        server and takes commands; the image's own program does not run.
        Defaults: 1 vCPU, 1024 MiB, seed 0, the image's environment, user
        and directory, and a name from the image."""
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
            created = await connection.json("POST", "/sandboxes", body=draft)
            sandbox = cls(connection, created, input_offset=0)
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

    async def run(
        self, command: list[str] | str, *, env: Optional[Mapping[str, str]] = None,
        cwd: Optional[str] = None, stdin: Optional[bytes] = None,
    ) -> CommandResult:
        """Runs argv, or a shell script, to its end. Stdin bytes are sent
        then closed; omitted stdin is empty. Nonzero codes are results.
        """
        handle = await self.start(
            command, env=env, cwd=cwd, stdin=b"" if stdin is None else stdin,
        )
        return await handle.wait()

    async def start(
        self, command: list[str] | str, *, env: Optional[Mapping[str, str]] = None,
        cwd: Optional[str] = None, stdin: Optional[bytes] = None,
    ) -> AsyncCommand:
        """Starts argv or a shell script and returns its handle. Supplied
        stdin bytes are sent then closed; otherwise the handle owns stdin.
        """
        return await self._command_session().start(command, env=env, cwd=cwd, stdin=stdin)

    async def write_file(self, path: str, data: bytes, *, mode: Optional[int] = None) -> None:
        """Writes a whole file in pieces; mode defaults to 0o644, subject
        to umask. Parent directories must exist; writes are not atomic.
        """
        await self._command_session().write_file(path, data, mode)

    async def read_file(self, path: str) -> bytes:
        """Reads a whole regular file as bytes, in protocol pieces."""
        return await self._command_session().read_file(path)

    async def end(self) -> int:
        """Ends a command sandbox's input and waits for its exit code.
        The server kills commands still running. Resume a paused sandbox first.
        """
        return await self._command_session().end()

    def _command_session(self) -> SandboxSession:
        if not self._takes_commands:
            raise DvmError("this sandbox runs a supplied program and does not take commands")
        if self._session is None:
            self._session = SandboxSession(
                self._connection, self.id, self.wait, self._input_offset, self._ended_error,
            )
        return self._session

    async def output(self) -> Output:
        """What process 1 wrote so far. For a sandbox that takes commands,
        stdout is the server's raw JSON answer lines, not command output.
        """
        return Output.from_api(await self._connection.json("GET", f"/executions/{self.id}/output"))

    async def close(self) -> None:
        """Closes its connection; the sandbox goes on."""
        if self._session is not None:
            await self._session.close()
        await self._connection.close()

    async def __aenter__(self) -> AsyncSandbox:
        return self

    async def __aexit__(self, *_: object) -> None:
        try:
            state = await self.state()
            if state.is_active:
                if self._takes_commands and state.kind != "paused":
                    await self.end()
                else:
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
