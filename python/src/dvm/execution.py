"""An execution: everything a sandbox ran, one log from its start to its
end, under the sandbox's id; and its replays, each judged the same
execution or not."""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any, Iterable, Mapping, Optional, Union

from dvm.commands import CommandInfo
from dvm.connection import Connection
from dvm.errors import ReplayEndedError
from dvm.run_state import RunState

#: A moment of the execution: ``"<thread>:<operation>"``, before that
#: thread's operation, or ``"@<milliseconds>"`` into it, as dvm writes them.
Moment = str


@dataclass(frozen=True)
class Verdict:
    """Whether a replay was its execution, as far as can be seen from
    outside: ``same``; ``other_exit`` (``recorded`` and ``replayed`` codes);
    ``other_output`` (the ``stream``, and the byte ``at`` which it parted);
    or ``other_command``. ``command`` is the command it parted in."""

    kind: str
    command: Optional[int] = None
    stream: Optional[str] = None
    at: Optional[int] = None
    recorded: Optional[int] = None
    replayed: Optional[int] = None

    @classmethod
    def from_api(cls, verdict: Mapping[str, Any]) -> Verdict:
        return cls(**verdict)

    @property
    def same(self) -> bool:
        return self.kind == "same"


class AsyncExecution:
    """A sandbox's execution, from ``await AsyncExecution.get(sandbox_id)``:
    its ``spec`` (image, seed, CPUs, memory, variables), its ``state``, each
    command it was sent, as it ran, and its log's size once it ended."""

    def __init__(self, connection: Connection, execution: Mapping[str, Any]) -> None:
        self._connection = connection
        self.id: int = execution["id"]
        self.spec: Mapping[str, Any] = execution["spec"]
        run = execution["run"]
        self.state = RunState.from_api(run["state"])
        self.log_bytes: Optional[int] = run["log_bytes"]
        self.commands = [CommandInfo.from_api(command) for command in execution["commands"]]
        self.replays = [AsyncReplay(connection, replay) for replay in execution["replays"]]

    @classmethod
    async def get(
        cls,
        execution_id: int,
        *,
        api_key: Optional[str] = None,
        api_url: Optional[str] = None,
    ) -> AsyncExecution:
        connection = Connection(api_key, api_url)
        try:
            return cls(connection, await connection.json("GET", f"/executions/{execution_id}"))
        except BaseException:
            await connection.close()
            raise

    async def replay(self, *, watch: Iterable[str] = (), held: bool = False) -> AsyncReplay:
        """Replays it, once its sandbox ended, played to its end, watching
        the events ``watch`` names (``files``, ``network``, ``processes``,
        ``memory``, ``time``, ``signals``, ``waits``, ``other``,
        ``page-faults``); or ``held`` at its start, for ``seek``."""
        new = {"held": held, "watch": list(watch)}
        replay = await self._connection.json("POST", f"/executions/{self.id}/replays", body=new)
        return AsyncReplay(self._connection, replay)

    async def machine(self, at: Moment) -> Mapping[str, Any]:
        """The machine at the moment ``at``: each process, its threads,
        files and memory, as dvm describes them."""
        return await self._connection.json(
            "GET", f"/executions/{self.id}/machine", params={"at": at}
        )

    async def close(self) -> None:
        await self._connection.close()


class AsyncReplay:
    """A replay of an execution."""

    def __init__(self, connection: Connection, replay: Mapping[str, Any]) -> None:
        self._connection = connection
        self.id: int = replay["id"]
        self._path = f"/replays/{self.id}"
        self._take(replay)

    def _take(self, replay: Mapping[str, Any]) -> None:
        self._version: int = replay["version"]
        self.state = RunState.from_api(replay["state"])
        verdict = replay["verdict"]
        self.verdict: Optional[Verdict] = Verdict.from_api(verdict) if verdict else None

    async def wait(self) -> Verdict:
        """Its verdict, once it ended: the same execution, or where it
        parted. A replay dvm failed, as it saw it part, raises where."""
        while self.state.is_active:
            replay = await self._connection.json(
                "GET", self._path, params={"newer_than": self._version}
            )
            self._take(replay)
        if self.verdict is None:
            raise ReplayEndedError(self.id, self.state)
        return self.verdict

    async def seek(self, to: Moment) -> Mapping[str, Any]:
        """Takes a held replay to the moment ``to``, and holds it there: the
        moment, each thread's next operation, and the machine there."""
        return await self._connection.json(
            "POST", f"{self._path}/seek", body={"to": _moment_of(to)}
        )

    async def stop(self) -> None:
        self._take(await self._connection.json("POST", f"{self._path}/stop"))

    async def commands(self) -> list[CommandInfo]:
        """The commands it ran again, oldest first."""
        commands = await self._connection.json("GET", f"{self._path}/commands")
        return [CommandInfo.from_api(command) for command in commands]

    async def events(self) -> Mapping[str, Any]:
        """What it watched, and the events it saw, once it ended."""
        return await self._connection.json("GET", f"{self._path}/events")


def _moment_of(moment: Union[Moment, Mapping[str, Any]]) -> Mapping[str, Any]:
    """A moment as dvm writes it, as the API takes it."""
    if not isinstance(moment, str):
        return moment
    if moment.startswith("@"):
        whole, _, fraction = moment[1:].partition(".")
        at_us = int(whole) * 1000 + int((fraction + "000")[:3])
        return {"kind": "time", "at_us": at_us}
    thread, _, operation = moment.partition(":")
    return {"kind": "event", "thread": int(thread), "operation": int(operation)}
