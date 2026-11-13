"""The program lifecycle against the platform."""
import asyncio
import os
import uuid
import pytest
from conftest import IMAGE
from dvm import (AsyncExecution, AsyncSandbox, AuthenticationError, ConflictError,
                 DvmError, Execution, InvalidRequestError, Sandbox, SandboxEndedError)

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


def test_a_command_session_ends_with_a_whole_recording():
    path = f"/tmp/dvm-python-{uuid.uuid4().hex}"
    with Sandbox.create(IMAGE, env={"A": "inherited"}) as sandbox:
        result = sandbox.run(["sh", "-c", "printf '%s ' \"$A\"; pwd; printf err >&2"],
                             env={"A": "command"}, cwd="/tmp")
        assert (result.exit_code, result.stdout, result.stderr) == (0, b"command /tmp\n", b"err")
        assert sandbox.run("exit 3").exit_code == 3
        assert sandbox.run("printf '%s' \"$A\"").stdout == b"inherited"
        assert sandbox.run(["cat"], stdin=b"hi\x00\xff\n").stdout == b"hi\x00\xff\n"
        data = bytes(range(256)) * 400
        sandbox.write_file(path, data, mode=0o600)
        assert sandbox.read_file(path) == data
        server = sandbox.start(["sh", "-c", "while :; do :; done"])
        assert sandbox.run("printf concurrent").stdout == b"concurrent"
        server.signal(15)
        assert server.wait().exit_code == 143
        cat = sandbox.start(["cat"])
        cat.write_stdin(b"from the handle")
        cat.close_stdin()
        assert cat.wait().stdout == b"from the handle"
        sandbox.run(["rm", path])
        assert b'"started"' in sandbox.output().stdout
        connected = Sandbox.connect(sandbox.id)
        assert connected.run("printf connected").stdout == b"connected"
        connected.close()
    execution = Execution.get(sandbox.id)
    assert (execution.state.kind, execution.state.exit_code) == ("exited", 0)
    if os.environ.get("DVM_TEST_SESSION_REPLAY") == "1":
        assert execution.replay().wait().same
    execution.close()


def test_new_methods_refuse_a_sandbox_with_a_command():
    with Sandbox.create(IMAGE, command=["true"]) as sandbox:
        for call in (lambda: sandbox.run("true"), lambda: sandbox.start("true"),
                     lambda: sandbox.write_file("/tmp/a", b"hi"),
                     lambda: sandbox.read_file("/tmp/a"), sandbox.end):
            with pytest.raises(DvmError, match="supplied program"):
                call()


@pytest.mark.skipif(os.environ.get("DVM_TEST_IN_DVM") != "1",
                    reason="users and the network are dvm's; a stand-in has neither")
def test_commands_run_as_a_user_and_the_network_changes_as_the_sandbox_runs():
    """The run's resolver is reached over TCP, as the runner's own takes it,
    unless the network is none; link-local addresses never are."""
    resolver = "nc 172.17.0.1 53 </dev/null"
    with Sandbox.create(IMAGE) as sandbox:
        assert sandbox.run("id -u").stdout == b"0\n"
        assert sandbox.run("id -u; echo $HOME", user="nobody").stdout == b"65534\n/\n"
        assert sandbox.run(resolver).exit_code == 0
        sandbox.set_network("none")
        assert sandbox.run(resolver).exit_code != 0
        sandbox.set_network("allowlist", ["pypi.org"])
        assert sandbox.run(resolver).exit_code == 0
        with pytest.raises(InvalidRequestError):
            sandbox.set_network("allowlist", ["169.254.169.254"])
        metadata = sandbox.run("wget -q -T 5 -O - http://169.254.169.254/ 2>&1")
        assert b"unreachable" in metadata.stdout
        execution = Execution.get(sandbox.id)
        assert execution.spec["network"] == {"mode": "allowlist", "allow": ["pypi.org"]}
        execution.close()
