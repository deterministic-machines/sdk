"""The SDK against dvm.run, or a controller and a runner, at DVM_API_URL
with DVM_API_KEY: dvm-platform's tests start them, and run these."""

import asyncio

import pytest

from conftest import IMAGE
from dvm import (
    AsyncExecution,
    AsyncSandbox,
    AuthenticationError,
    ConflictError,
    Execution,
    InvalidRequestError,
    NotFoundError,
    Sandbox,
    SandboxEndedError,
)

pytestmark = pytest.mark.platform


def test_a_command_returns_its_output_and_exit_code(sandbox):
    script = sandbox.commands.run("echo out; echo err >&2; exit 3")
    assert (script.stdout, script.stderr, script.exit_code, script.signal) == (
        "out\n",
        "err\n",
        3,
        None,
    )
    assert sandbox.commands.run(["echo", "a  b"]).stdout == "a  b\n"
    assert sandbox.commands.run("echo $A", env={"A": "1"}).stdout == "1\n"


def test_output_comes_to_callbacks_as_it_is_written(sandbox):
    pieces = []
    counted = sandbox.commands.run(
        "for i in 1 2 3; do echo $i; sleep 0.02; done", on_stdout=pieces.append
    )
    assert "".join(pieces) == counted.stdout == "1\n2\n3\n"


def test_a_background_command_takes_stdin_until_its_close(sandbox):
    cat = sandbox.commands.run(["cat"], background=True, stdin=True)
    cat.send_stdin(b"one\n")
    cat.send_stdin("two\n")
    cat.close_stdin()
    assert (cat.wait().stdout, cat.info().state) == ("one\ntwo\n", "exited")
    assert sandbox.commands.run(["cat"], stdin="piped").stdout == "piped"


def test_a_killed_command_says_its_signal(sandbox):
    sleeper = sandbox.commands.run(["sleep", "30"], background=True)
    sleeper.kill()
    killed = sleeper.wait()
    assert (killed.exit_code, killed.signal) == (137, 9)
    timed_out = sandbox.commands.run(["sleep", "30"], timeout=0.2)
    assert (timed_out.exit_code, timed_out.signal) == (137, 9)


def test_a_file_goes_in_and_out(sandbox):
    data = bytes(range(256))
    sandbox.files.write("/data.bin", data)
    assert sandbox.files.read("/data.bin") == data
    sandbox.files.write("/run.sh", "#!/bin/sh\necho ran\n", mode=0o755)
    assert sandbox.commands.run("./run.sh").stdout == "ran\n"
    with pytest.raises(NotFoundError):
        sandbox.files.read("/missing")


def test_a_paused_sandbox_takes_no_commands_until_resumed(sandbox):
    sandbox.pause()
    assert sandbox.state().kind == "paused"
    with pytest.raises(ConflictError):
        sandbox.commands.run("true")
    sandbox.resume()
    assert sandbox.commands.run("echo back").stdout == "back\n"


def test_another_process_connects_by_id(sandbox):
    connected = Sandbox.connect(sandbox.id)
    assert connected.commands.run("echo here").stdout == "here\n"
    connected.close()


def test_a_stopped_sandbox_says_it_ended():
    sandbox = Sandbox.create(IMAGE, name="stopped")
    assert sandbox.name == "stopped"
    sandbox.stop()
    sandbox.stop()
    with pytest.raises(SandboxEndedError) as ended:
        sandbox.commands.run("true")
    assert ended.value.state.kind == "stopped"
    sandbox.close()


def test_an_execution_replays_the_same():
    with Sandbox.create(IMAGE, seed=7) as sandbox:
        sandbox.files.write("/note.txt", "hello\n")
        assert sandbox.commands.run("cat note.txt").stdout == "hello\n"
        assert sandbox.commands.run("exit 2").exit_code == 2
    execution = Execution.get(sandbox.id)
    assert (execution.state.kind, execution.spec["seed"], execution.spec["name"]) == (
        "stopped",
        7,
        "alpine",
    )
    assert execution.log_bytes is not None
    assert [(c.number, c.state, c.exit_code) for c in execution.commands] == [
        (1, "exited", 0),
        (2, "exited", 2),
    ]
    assert execution.replay().wait().same
    execution.close()


def test_the_async_api_is_the_same():
    async def run_and_replay():
        async with await AsyncSandbox.create(IMAGE) as sandbox:
            result = await sandbox.commands.run("echo async")
        execution = await AsyncExecution.get(sandbox.id)
        verdict = await (await execution.replay()).wait()
        await execution.close()
        return result.stdout, verdict.same

    assert asyncio.run(run_and_replay()) == ("async\n", True)


def test_refusals_are_typed():
    with pytest.raises(InvalidRequestError):
        Sandbox.create("Not An Image")
    with pytest.raises(AuthenticationError):
        Sandbox.create(IMAGE, api_key="dvm_not-a-key")
