"""The program lifecycle against the platform."""
import asyncio
import pytest
from conftest import IMAGE
from dvm import (AsyncExecution, AsyncSandbox, AuthenticationError, ConflictError,
                 Execution, InvalidRequestError, Sandbox, SandboxEndedError)

pytestmark = pytest.mark.platform


def test_a_program_returns_its_output_and_exit_code():
    with Sandbox.create(IMAGE, command=["sh", "-c", "printf 'out\\377'; echo err >&2; exit 3"], seed=7) as sandbox:
        assert sandbox.wait() == 3
        output = sandbox.output()
        assert (output.stdout, output.stderr) == (b"out\xfferr\n", b"")
    execution = Execution.get(sandbox.id)
    assert (execution.state.kind, execution.spec["seed"], execution.spec["name"]) == ("exited", 7, "alpine")
    assert execution.log_bytes is not None
    replay = execution.replay()
    assert replay.wait().same
    assert replay.output() == output
    execution.close()


def test_entrypoint_and_environment():
    with Sandbox.create(IMAGE, entrypoint="sh", command=["-c", "printf '%s' \"$A\""], env={"A": "a  b"}) as sandbox:
        assert sandbox.wait() == 0
        assert sandbox.output().stdout == b"a  b"


def test_an_image_runs_its_own_command():
    with Sandbox.create(IMAGE) as sandbox:
        assert sandbox.wait() == 0


def test_pause_resume_connect_and_stop():
    with Sandbox.create(IMAGE, command=["sh", "-c", "while :; do :; done"], name="stopped") as sandbox:
        connected = Sandbox.connect(sandbox.id)
        connected.pause()
        assert sandbox.state().kind == "paused"
        connected.resume()
        assert sandbox.state().kind == "running"
        connected.stop()
        connected.stop()
        with pytest.raises(SandboxEndedError):
            sandbox.wait()
        connected.close()
    execution = Execution.get(sandbox.id)
    with pytest.raises(ConflictError):
        execution.replay()
    execution.close()


def test_the_async_api_is_the_same():
    async def run():
        async with await AsyncSandbox.create(IMAGE, command=["echo", "async"]) as sandbox:
            assert await sandbox.wait() == 0
            assert (await sandbox.output()).stdout == b"async\n"
        execution = await AsyncExecution.get(sandbox.id)
        assert (await (await execution.replay()).wait()).same
        await execution.close()
    asyncio.run(run())


def test_refusals_are_typed():
    with pytest.raises(InvalidRequestError):
        Sandbox.create("Not An Image")
    with pytest.raises(AuthenticationError):
        Sandbox.create(IMAGE, api_key="dvm_not-a-key")
