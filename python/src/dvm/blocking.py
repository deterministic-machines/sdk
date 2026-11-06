"""The blocking API: the async one, each call run to its end on an event
loop of the SDK's own, in a thread, so that it works from any code, an
event loop's too. Output callbacks are called in that thread."""

from __future__ import annotations

import asyncio
import threading
from typing import Any, Coroutine, Iterable, Mapping, Optional, TypeVar, Union

from dvm.commands import (
    AsyncCommandHandle,
    AsyncCommands,
    CommandInfo,
    CommandLine,
    CommandResult,
    OutputCallback,
)
from dvm.execution import AsyncExecution, AsyncReplay, Moment, Verdict
from dvm.files import AsyncFiles
from dvm.run_state import RunState
from dvm.sandbox import AsyncSandbox

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
    from ``Sandbox.connect(sandbox_id)``. Its ``id`` is its execution's too.
    As a context manager, it's stopped at the end."""

    def __init__(self, sandbox: AsyncSandbox) -> None:
        self._sandbox = sandbox
        self.id = sandbox.id
        self.name = sandbox.name
        self.commands = Commands(sandbox.commands)
        self.files = Files(sandbox.files)

    @classmethod
    def create(
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
    ) -> Sandbox:
        """A sandbox of ``image``, once it's running (:meth:`AsyncSandbox.create`)."""
        return cls(
            _run(
                AsyncSandbox.create(
                    image,
                    cpus=cpus,
                    memory_mib=memory_mib,
                    seed=seed,
                    env=env,
                    name=name,
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

    def resume(self) -> None:
        _run(self._sandbox.resume())

    def stop(self) -> None:
        """Stops it, and returns once its execution replays (:meth:`AsyncSandbox.stop`)."""
        _run(self._sandbox.stop())

    def close(self) -> None:
        _run(self._sandbox.close())

    def __enter__(self) -> Sandbox:
        return self

    def __exit__(self, *_: object) -> None:
        _run(self._sandbox.__aexit__())


class Commands:
    """A sandbox's commands: ``sandbox.commands``."""

    def __init__(self, commands: AsyncCommands) -> None:
        self._commands = commands

    def run(
        self,
        command: CommandLine,
        *,
        background: bool = False,
        cwd: Optional[str] = None,
        env: Optional[Mapping[str, str]] = None,
        stdin: Union[bool, bytes, str] = False,
        timeout: Optional[float] = None,
        on_stdout: Optional[OutputCallback] = None,
        on_stderr: Optional[OutputCallback] = None,
    ) -> Union[CommandResult, CommandHandle]:
        """Runs ``command`` to its exit, or starts it with
        ``background=True`` (:meth:`AsyncCommands.run`)."""
        ran = _run(
            self._commands.run(
                command,
                background=background,
                cwd=cwd,
                env=env,
                stdin=stdin,
                timeout=timeout,
                on_stdout=on_stdout,
                on_stderr=on_stderr,
            )
        )
        return CommandHandle(ran) if isinstance(ran, AsyncCommandHandle) else ran


class CommandHandle:
    """A command started in the background."""

    def __init__(self, handle: AsyncCommandHandle) -> None:
        self._handle = handle
        self.number = handle.number

    def wait(
        self,
        *,
        timeout: Optional[float] = None,
        on_stdout: Optional[OutputCallback] = None,
        on_stderr: Optional[OutputCallback] = None,
    ) -> CommandResult:
        return _run(self._handle.wait(timeout=timeout, on_stdout=on_stdout, on_stderr=on_stderr))

    def send_stdin(self, data: Union[bytes, str], *, close: bool = False) -> None:
        _run(self._handle.send_stdin(data, close=close))

    def close_stdin(self) -> None:
        _run(self._handle.close_stdin())

    def signal(self, signal: int) -> None:
        _run(self._handle.signal(signal))

    def kill(self) -> None:
        _run(self._handle.kill())

    def info(self) -> CommandInfo:
        return _run(self._handle.info())


class Files:
    """A sandbox's files: ``sandbox.files``."""

    def __init__(self, files: AsyncFiles) -> None:
        self._files = files

    def write(self, path: str, data: Union[bytes, str], *, mode: Optional[int] = None) -> None:
        _run(self._files.write(path, data, mode=mode))

    def read(self, path: str) -> bytes:
        return _run(self._files.read(path))

    def read_text(self, path: str, encoding: str = "utf-8") -> str:
        return _run(self._files.read_text(path, encoding))


class Execution:
    """A sandbox's execution, from ``Execution.get(sandbox_id)`` (:class:`AsyncExecution`)."""

    def __init__(self, execution: AsyncExecution) -> None:
        self._execution = execution
        self.id = execution.id
        self.spec = execution.spec
        self.state = execution.state
        self.log_bytes = execution.log_bytes
        self.commands = execution.commands
        self.replays = [Replay(replay) for replay in execution.replays]

    @classmethod
    def get(
        cls,
        execution_id: int,
        *,
        api_key: Optional[str] = None,
        api_url: Optional[str] = None,
    ) -> Execution:
        return cls(_run(AsyncExecution.get(execution_id, api_key=api_key, api_url=api_url)))

    def replay(self, *, watch: Iterable[str] = (), held: bool = False) -> Replay:
        return Replay(_run(self._execution.replay(watch=watch, held=held)))

    def machine(self, at: Moment) -> Mapping[str, Any]:
        return _run(self._execution.machine(at))

    def close(self) -> None:
        _run(self._execution.close())


class Replay:
    """A replay of an execution (:class:`AsyncReplay`)."""

    def __init__(self, replay: AsyncReplay) -> None:
        self._replay = replay
        self.id = replay.id

    @property
    def state(self) -> RunState:
        return self._replay.state

    @property
    def verdict(self) -> Optional[Verdict]:
        return self._replay.verdict

    def wait(self) -> Verdict:
        return _run(self._replay.wait())

    def seek(self, to: Moment) -> Mapping[str, Any]:
        return _run(self._replay.seek(to))

    def stop(self) -> None:
        _run(self._replay.stop())

    def commands(self) -> list[CommandInfo]:
        return _run(self._replay.commands())

    def events(self) -> Mapping[str, Any]:
        return _run(self._replay.events())
