"""Provider contracts against an SDK fake; no platform or Docker required."""

import asyncio
import io
import re
import signal
import subprocess
import tarfile
from pathlib import Path

import pytest

pytest.importorskip("harbor")

from harbor.environments.factory import EnvironmentFactory
from harbor.models.task.config import EnvironmentConfig, NetworkPolicy
from harbor.models.trial.paths import TrialPaths

from dvm import CommandResult
from dvm.harbor import DvmEnvironment, sandbox_name


class FakeCommand:
    def __init__(self, result):
        self.result = result
        self.signals = []
        self.killed = asyncio.Event()
        self.waiting = asyncio.Event()

    async def wait(self):
        self.waiting.set()
        if self.result is None:
            await self.killed.wait()
            return CommandResult(137, b"", b"")
        return self.result

    async def signal(self, number):
        self.signals.append(number)
        self.killed.set()


class FakeSandbox:
    def __init__(self, root):
        self.id = 42
        self.root = root
        self.calls = []
        self.result = CommandResult(7, b"out\xff", b"err")
        self.handle = None
        self.started = asyncio.Event()
        self.end_error = None

    def local(self, path):
        return self.root / path.lstrip("/")

    async def run(self, command, **kwargs):
        self.calls.append(("run", command, kwargs))
        if isinstance(command, list):
            args = [str(self.local(arg)) if arg.startswith("/") else arg for arg in command]
            result = subprocess.run(args, cwd=self.root, capture_output=True)
            return CommandResult(result.returncode, result.stdout, result.stderr)
        return self.result

    async def start(self, command, **kwargs):
        self.calls.append(("start", command, kwargs))
        self.handle = FakeCommand(self.result)
        self.started.set()
        return self.handle

    async def write_file(self, path, data, *, mode=None):
        self.calls.append(("write_file", path))
        target = self.local(path)
        target.write_bytes(data)
        if mode is not None:
            target.chmod(mode)

    async def read_file(self, path):
        self.calls.append(("read_file", path))
        return self.local(path).read_bytes()

    async def set_network(self, **kwargs):
        self.calls.append(("set_network", kwargs))

    async def end(self):
        self.calls.append(("end",))
        if self.end_error:
            raise self.end_error
        return 0

    async def stop(self):
        self.calls.append(("stop",))

    async def close(self):
        self.calls.append(("close",))


@pytest.fixture
def provider(tmp_path, monkeypatch):
    root = tmp_path / "sandbox"
    (root / "tmp").mkdir(parents=True)
    sandbox = FakeSandbox(root)
    created = []

    async def create(image, **kwargs):
        created.append((image, kwargs))
        return sandbox

    monkeypatch.setattr("dvm.harbor.AsyncSandbox.create", create)
    environment_dir = tmp_path / "environment"
    environment_dir.mkdir()

    def make(config=None, **kwargs):
        return DvmEnvironment(
            environment_dir=environment_dir,
            environment_name="tiny-task",
            session_id=kwargs.pop("session_id", "tiny-task__trial__env"),
            trial_paths=TrialPaths(tmp_path / "trial"),
            task_env_config=EnvironmentConfig(**({"docker_image": "python:3.12-slim"} | (config or {}))),
            **kwargs,
        )

    return make, sandbox, created


def test_harbor_loads_the_provider_by_import_path():
    assert EnvironmentFactory.get_environment_class(import_path="dvm.harbor:DvmEnvironment") is DvmEnvironment
    assert not DvmEnvironment.__abstractmethods__


@pytest.mark.parametrize("session,role", [
    ("tiny-task__trial__env", "agent"),
    ("tiny-task__trial__verifier__grade", "grading"),
])
def test_start_uses_the_image_resources_network_and_role(provider, session, role):
    make, sandbox, created = provider
    environment = make(
        {"cpus": 2, "memory_mb": 4096, "network_mode": "no-network", "env": {"TASK": "yes"}},
        session_id=session,
    )
    asyncio.run(environment.start())
    assert created == [("python:3.12-slim", {
        "cpus": 2, "memory_mib": 4096, "env": {"TASK": "yes"},
        "name": f"tiny-task-{role}", "network": "none",
    })]
    assert environment.sandbox_id == sandbox.id


@pytest.mark.parametrize("task, role, name", [
    ("layout-config-recreation2", "agent", "layout-config-recreation2-agent"),
    ("terminal-bench/Layout_Config", "grading", "terminal-bench-layout-config-grading"),
    ("-" * 3, "agent", "task-agent"),
    ("a" * 80, "grading", "a" * 55 + "-grading"),
])
def test_sandbox_names_follow_the_platform_rule(task, role, name):
    assert sandbox_name(task, role) == name
    assert re.fullmatch(r"[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?", name)


@pytest.mark.parametrize("timeout", [None, 17])
def test_exec_forwards_command_cwd_env_user_and_timeout(provider, monkeypatch, timeout):
    make, sandbox, _ = provider
    environment = make({"workdir": "/image", "env": {"BASE": "task"}})
    timeouts = []
    original = asyncio.timeout

    def record_timeout(delay):
        timeouts.append(delay)
        return original(delay)

    monkeypatch.setattr("dvm.harbor.asyncio.timeout", record_timeout)

    async def run():
        await environment.start()
        sandbox.calls.clear()
        with environment.scoped_exec_env({"SCOPE": "yes"}):
            return await environment.exec("echo hello", cwd="/app", env={"BASE": "command"}, user=123, timeout_sec=timeout)

    result = asyncio.run(run())
    assert (result.stdout, result.stderr, result.return_code) == ("out\ufffd", "err", 7)
    options = {"cwd": "/app", "env": {"BASE": "command", "SCOPE": "yes"}, "user": "123"}
    if timeout is None:
        assert sandbox.calls == [("run", "echo hello", options)]
        assert timeouts == []
    else:
        assert sandbox.calls == [("start", "echo hello", {"stdin": b"", **options})]
        assert timeouts == [timeout]


def test_exec_uses_harbors_default_user_and_workdir(provider):
    make, sandbox, _ = provider
    environment = make({"workdir": "/app"})

    async def run():
        await environment.start()
        with environment.with_default_user("nobody"):
            await environment.exec("id")

    asyncio.run(run())
    assert sandbox.calls[-1] == ("run", "id", {"cwd": "/app", "env": None, "user": "nobody"})


def test_timeout_kills_the_command_even_during_dispatch(provider, monkeypatch):
    make, sandbox, _ = provider
    environment = make()
    original = asyncio.timeout
    monkeypatch.setattr("dvm.harbor.asyncio.timeout", lambda delay: original(0 if delay == 17 else delay))

    async def run():
        await environment.start()
        sandbox.result = None
        with pytest.raises(TimeoutError):
            await environment.exec("never finishes", timeout_sec=17)
        assert sandbox.handle.signals == [signal.SIGKILL]
        assert ("stop",) not in sandbox.calls

    asyncio.run(run())


def test_cancelling_exec_kills_the_running_command(provider):
    make, sandbox, _ = provider
    environment = make()

    async def run():
        await environment.start()
        sandbox.result = None
        task = asyncio.create_task(environment.exec("never finishes", timeout_sec=17))
        await sandbox.started.wait()
        await sandbox.handle.waiting.wait()
        task.cancel()
        with pytest.raises(asyncio.CancelledError):
            await task
        assert sandbox.handle.signals == [signal.SIGKILL]

    asyncio.run(run())


def test_directories_round_trip_with_modes_links_empty_dirs_and_binary_files(provider, tmp_path):
    make, sandbox, _ = provider
    source = tmp_path / "source"
    (source / "empty").mkdir(parents=True)
    (source / "nested").mkdir()
    (source / "nested" / "binary").write_bytes(bytes(range(256)))
    (source / "solve.sh").write_text("#!/bin/sh\necho yes\n")
    (source / "solve.sh").chmod(0o755)
    (source / "link").symlink_to("nested/binary")
    target = tmp_path / "download"

    async def run():
        environment = make()
        await environment.start()
        await environment.upload_dir(source, "/a directory with 'quotes'")
        await environment.download_dir("/a directory with 'quotes'", target)

    asyncio.run(run())
    assert (target / "nested" / "binary").read_bytes() == bytes(range(256))
    assert (target / "link").readlink() == Path("nested/binary")
    assert (target / "empty").is_dir()
    assert (target / "solve.sh").stat().st_mode & 0o777 == 0o755
    assert not list((sandbox.root / "tmp").iterdir())


def test_files_round_trip_and_create_parents(provider, tmp_path):
    make, sandbox, _ = provider
    source = tmp_path / "solve.sh"
    source.write_bytes(b"hello\x00\xff")
    source.chmod(0o755)
    target = tmp_path / "nested" / "download"

    async def run():
        environment = make()
        await environment.start()
        await environment.upload_file(source, "/new/solve.sh")
        await environment.download_file("/new/solve.sh", target)

    asyncio.run(run())
    assert target.read_bytes() == source.read_bytes()
    assert sandbox.local("/new/solve.sh").stat().st_mode & 0o777 == 0o755


def test_download_skips_unsafe_paths_and_keeps_the_rest(provider, tmp_path, monkeypatch):
    make, sandbox, _ = provider
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w") as archive:
        member = tarfile.TarInfo("../escape")
        member.size = 1
        archive.addfile(member, io.BytesIO(b"x"))
        member = tarfile.TarInfo("safe")
        member.size = 1
        archive.addfile(member, io.BytesIO(b"y"))

    async def read_file(path):
        return buffer.getvalue()

    async def run():
        environment = make()
        await environment.start()
        sandbox.local("/source").mkdir()
        monkeypatch.setattr(sandbox, "read_file", read_file)
        await environment.download_dir("/source", tmp_path / "download")

    asyncio.run(run())
    assert not (tmp_path / "escape").exists()
    assert (tmp_path / "download" / "safe").read_bytes() == b"y"


@pytest.mark.parametrize("delete", [True, False])
def test_stop_ends_input_and_closes_without_stopping_the_recording(provider, delete):
    make, sandbox, _ = provider
    environment = make()

    async def run():
        await environment.start()
        sandbox.calls.clear()
        await environment.stop(delete=delete)
        await environment.stop(delete=delete)

    asyncio.run(run())
    assert sandbox.calls == [("end",), ("close",)]
    assert environment.sandbox_id == 42


def test_stop_falls_back_only_when_end_fails(provider):
    make, sandbox, _ = provider
    sandbox.end_error = RuntimeError("cannot end")

    async def run():
        environment = make()
        await environment.start()
        sandbox.calls.clear()
        await environment.stop()

    asyncio.run(run())
    assert sandbox.calls == [("end",), ("stop",), ("close",)]


def test_network_capabilities_and_live_policy_changes(provider):
    make, sandbox, _ = provider
    environment = make()
    hosts = ["pypi.org", "*.github.com", "8.8.8.8", "8.8.8.0/24"]

    async def run():
        await environment.start()
        for mode in ("allowlist", "no-network", "public"):
            await environment.set_network_policy(NetworkPolicy(network_mode=mode, allowed_hosts=hosts if mode == "allowlist" else []))

    asyncio.run(run())
    assert [call for call in sandbox.calls if call[0] == "set_network"] == [
        ("set_network", {"network": "allowlist", "allow": hosts}),
        ("set_network", {"network": "none"}),
        ("set_network", {"network": "public"}),
    ]
    assert environment.capabilities.network_allowlist_ipv4_cidrs
    assert not environment.capabilities.network_allowlist_ipv6_addresses
    assert not environment.capabilities.network_allowlist_ipv6_cidrs


@pytest.mark.parametrize("config,message", [
    ({"docker_image": None}, "cannot build a Dockerfile"),
    ({"gpus": 1}, "GPU"),
    ({"tpu": {"type": "v5p", "topology": "2x2x1"}}, "TPU"),
    ({"os": "windows"}, "Windows"),
    ({"cpus": 0.5}, "whole CPU cores"),
    ({"network_mode": "allowlist", "allowed_hosts": ["2001:db8::1"]}, "IPv6"),
    ({"network_mode": "allowlist", "allowed_hosts": ["2001:db8::/32"]}, "IPv6"),
])
def test_unsupported_tasks_are_refused_before_create(provider, config, message):
    make, _, created = provider
    with pytest.raises((ValueError, RuntimeError), match=message):
        make(config)
    assert created == []


@pytest.mark.parametrize("filename", ["docker-compose.yaml", "docker-compose.yml", "compose.yaml", "compose.yml"])
def test_compose_is_refused_even_with_a_prebuilt_image(provider, tmp_path, filename):
    make, _, created = provider
    (tmp_path / "environment" / filename).write_text("services: {}")
    with pytest.raises(ValueError, match="Compose or multiple containers"):
        make()
    assert created == []


def test_force_build_is_refused_before_create(provider):
    make, _, created = provider
    with pytest.raises(ValueError, match="cannot build"):
        asyncio.run(make().start(force_build=True))
    assert created == []


def test_dockerfiles_cannot_supply_a_missing_image(provider, tmp_path):
    make, _, created = provider
    (tmp_path / "environment" / "Dockerfile").write_text("FROM python:3.12-slim\n")
    with pytest.raises(ValueError, match="cannot build a Dockerfile"):
        make({"docker_image": None})
    assert created == []


def test_runtime_policy_and_resource_overrides_take_precedence(provider):
    make, _, created = provider
    environment = make(
        {"cpus": 1, "memory_mb": 512, "network_mode": "public"},
        override_cpus=2, override_memory_mb=2048,
        network_policy=NetworkPolicy(network_mode="allowlist", allowed_hosts=["pypi.org"]),
    )
    asyncio.run(environment.start())
    options = created[0][1]
    assert (options["cpus"], options["memory_mib"]) == (2, 2048)
    assert (options["network"], options["allow"]) == ("allowlist", ["pypi.org"])


def test_unsupported_phase_policy_is_refused_before_create(provider):
    make, _, created = provider
    with pytest.raises(ValueError, match="IPv6"):
        make(phase_network_policies=[NetworkPolicy(network_mode="allowlist", allowed_hosts=["::1"])])
    assert created == []


def test_setup_failure_ends_and_closes_the_sandbox(provider, monkeypatch):
    make, sandbox, _ = provider
    environment = make()

    async def fail():
        raise RuntimeError("could not upload environment")

    monkeypatch.setattr(environment, "_upload_environment_dir_after_start", fail)
    with pytest.raises(RuntimeError, match="could not upload"):
        asyncio.run(environment.start())
    assert sandbox.calls[-2:] == [("end",), ("close",)]
