"""The blocking API: the async one, each call run to its end on an event
loop of the SDK's own, in a thread, so that it works from any code, an
event loop's too."""

from __future__ import annotations

import asyncio
import threading
from collections.abc import Sequence
from typing import Any, Coroutine, Mapping, Optional, TypeVar

from dvm.point import (
    AsyncSandboxPoint,
    DirEntry,
    FileStat,
    Output,
    StopEvent,
)
from dvm.run_state import RunState
from dvm.sandbox import AsyncSandbox
from dvm.sandbox_session import AsyncCommand, CommandResult

T = TypeVar("T")


class _EventLoopThread:
    """An event loop running in a daemon thread, started at its first call."""

    def __init__(self) -> None:
        self._loop: Optional[asyncio.AbstractEventLoop] = None
        self._starting = threading.Lock()

    def run(self, coroutine: Coroutine[Any, Any, T]) -> T:
        future = asyncio.run_coroutine_threadsafe(coroutine, self._started())
        try:
            return future.result()
        except BaseException:
            # A KeyboardInterrupt here leaves the call running in the loop.
            future.cancel()
            raise

    def _started(self) -> asyncio.AbstractEventLoop:
        with self._starting:
            if self._loop is None:
                loop = asyncio.new_event_loop()
                threading.Thread(target=loop.run_forever, name="dvm", daemon=True).start()
                self._loop = loop
            return self._loop


# One loop for the process: every connection lives on the loop it was made
# on, and a thread each would outlive each sandbox not closed.
_run = _EventLoopThread().run


class Sandbox:
    """A sandbox, from ``Sandbox.create(image)``, or another process's,
    from ``Sandbox.connect(sandbox_id)``. With no command or entrypoint it
    takes commands, and its context ends input and waits for exit. A paused
    sandbox or a supplied program is stopped at the end instead."""

    def __init__(self, sandbox: AsyncSandbox) -> None:
        self._sandbox = sandbox
        self.id = sandbox.id
        self.name = sandbox.name

    @property
    def spec(self) -> dict[str, Any]:
        return self._sandbox.spec

    @classmethod
    def create(
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
        network: Optional[str] = None,
        allow: Optional[Sequence[str]] = None,
        api_key: Optional[str] = None,
        api_url: Optional[str] = None,
    ) -> Sandbox:
        """A sandbox of ``image``, once it's running (:meth:`AsyncSandbox.create`)."""
        return cls(
            _run(
                AsyncSandbox.create(
                    image,
                    command=command,
                    entrypoint=entrypoint,
                    cpus=cpus,
                    memory_mib=memory_mib,
                    seed=seed,
                    env=env,
                    name=name,
                    network=network,
                    allow=allow,
                    api_key=api_key,
                    api_url=api_url,
                )
            )
        )

    @classmethod
    def connect(
        cls,
        sandbox_id: int,
        *,
        api_key: Optional[str] = None,
        api_url: Optional[str] = None,
    ) -> Sandbox:
        return cls(_run(AsyncSandbox.connect(sandbox_id, api_key=api_key, api_url=api_url)))

    def state(self) -> RunState:
        return _run(self._sandbox.state())

    def pause(self) -> None:
        _run(self._sandbox.pause())

    def set_network(self, network: str, allow: Optional[Sequence[str]] = None) -> None:
        """:meth:`AsyncSandbox.set_network`."""
        _run(self._sandbox.set_network(network, allow))

    def resume(self) -> None:
        _run(self._sandbox.resume())

    def stop(self) -> None:
        """Stops it if active."""
        _run(self._sandbox.stop())

    def wait(self) -> int:
        return _run(self._sandbox.wait())

    def run(
        self, command: list[str] | str, *, env: Optional[Mapping[str, str]] = None,
        cwd: Optional[str] = None, stdin: Optional[bytes] = None,
        user: Optional[str] = None,
    ) -> CommandResult:
        """Runs argv or a script to its end, with stdin sent then closed."""
        return _run(self._sandbox.run(command, env=env, cwd=cwd, stdin=stdin, user=user))

    def start(
        self, command: list[str] | str, *, env: Optional[Mapping[str, str]] = None,
        cwd: Optional[str] = None, stdin: Optional[bytes] = None,
        user: Optional[str] = None,
    ) -> Command:
        """Starts a command; omitted stdin stays open for the handle."""
        return Command(_run(
            self._sandbox.start(command, env=env, cwd=cwd, stdin=stdin, user=user)
        ))

    def write_file(self, path: str, data: bytes, *, mode: Optional[int] = None) -> None:
        _run(self._sandbox.write_file(path, data, mode=mode))

    def read_file(self, path: str) -> bytes:
        return _run(self._sandbox.read_file(path))

    def end(self) -> int:
        """Ends input and waits for a command sandbox's exit code."""
        return _run(self._sandbox.end())

    def output(self) -> Output:
        """Raw process output: server answer frames for a command sandbox."""
        return _run(self._sandbox.output())

    def seek(
        self,
        *,
        at_ns: Optional[int] = None,
        thread: Optional[int] = None,
        event: Optional[int] = None,
        watch: Optional[Sequence[Any]] = None,
    ) -> SandboxPoint:
        return SandboxPoint(
            _run(
                self._sandbox.seek(
                    at_ns=at_ns, thread=thread, event=event, watch=watch
                )
            )
        )

    def close(self) -> None:
        _run(self._sandbox.close())

    def __enter__(self) -> Sandbox:
        return self

    def __exit__(self, *_: object) -> None:
        _run(self._sandbox.__aexit__())


class SandboxPoint:
    """One point in a sandbox's execution history (:class:`AsyncSandboxPoint`)."""

    def __init__(self, point: AsyncSandboxPoint) -> None:
        self._point = point
        self.sandbox_id = point.sandbox_id

    @property
    def thread(self) -> int:
        return self._point.thread

    @property
    def process(self) -> int:
        return self._point.process

    @property
    def operations(self) -> int:
        return self._point.operations

    @property
    def event(self) -> int:
        return self._point.event

    @property
    def ns(self) -> Optional[int]:
        return self._point.ns

    @property
    def at(self) -> StopEvent:
        return self._point.at

    @property
    def stats(self) -> dict[str, int]:
        return self._point.stats

    def step(self, *, watch: Optional[Sequence[Any]] = None) -> Optional[SandboxPoint]:
        next_point = _run(self._point.step(watch=watch))
        return SandboxPoint(next_point) if next_point is not None else None

    def read_file(self, path: str) -> bytes:
        return _run(self._point.read_file(path))

    def list_dir(self, path: str) -> list[DirEntry]:
        return _run(self._point.list_dir(path))

    def stat(self, path: str, *, follow_symlinks: bool = True) -> FileStat:
        return _run(self._point.stat(path, follow_symlinks=follow_symlinks))


class Command:
    """A started command, with the same methods as ``AsyncCommand``."""

    def __init__(self, command: AsyncCommand) -> None:
        self._command = command

    def wait(self) -> CommandResult:
        return _run(self._command.wait())

    def signal(self, number: int) -> None:
        _run(self._command.signal(number))

    def write_stdin(self, data: bytes) -> None:
        _run(self._command.write_stdin(data))

    def close_stdin(self) -> None:
        _run(self._command.close_stdin())
