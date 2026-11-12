"""An execution: everything a sandbox ran, one log from its start to its
end, under the sandbox's id; and its replays, each judged the same
execution or not."""

from __future__ import annotations

import base64
from dataclasses import dataclass
from typing import Any, Mapping, Optional

from dvm.connection import Connection
from dvm.errors import ReplayEndedError
from dvm.run_state import RunState


@dataclass(frozen=True)
class Output:
    """What a run wrote, as bytes. The program's stdout and stderr are one
    stream in dvm, in the order it wrote them: ``stdout``. ``stderr`` is
    what dvm itself said."""

    stdout: bytes
    stderr: bytes

    @classmethod
    def from_api(cls, output: Mapping[str, Any]) -> Output:
        return cls(base64.b64decode(output["stdout"]), base64.b64decode(output["stderr"]))


@dataclass(frozen=True)
class Verdict:
    """Whether a replay was its execution, as far as can be seen from
    outside: ``same``; ``other_exit`` (``recorded`` and ``replayed`` codes);
    ``other_output`` (the ``stream``, and the byte ``at`` which it parted)."""

    kind: str
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
    replay, and its log's size once it ended."""

    def __init__(self, connection: Connection, execution: Mapping[str, Any]) -> None:
        self._connection = connection
        self.id: int = execution["id"]
        self.spec: Mapping[str, Any] = execution["spec"]
        run = execution["run"]
        self.state = RunState.from_api(run["state"])
        self.log_bytes: Optional[int] = run["log_bytes"]
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

    async def replay(self) -> AsyncReplay:
        """Replays the exited program from its whole recording."""
        replay = await self._connection.json("POST", f"/executions/{self.id}/replays")
        return AsyncReplay(self._connection, replay)

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

    async def stop(self) -> None:
        self._take(await self._connection.json("POST", f"{self._path}/stop"))

    async def output(self) -> Output:
        """What the replayed program wrote so far (``Output``)."""
        return Output.from_api(await self._connection.json("GET", f"{self._path}/output"))
