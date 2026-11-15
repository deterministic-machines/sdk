"""Harbor's environment provider for recorded dvm.run command sandboxes.

Loaded by Harbor with ``--env dvm.harbor:DvmEnvironment``. Harbor belongs
to the caller's installation, not to the SDK's dependencies.
"""

from __future__ import annotations

import asyncio
import io
import re
import signal
import tarfile
from contextlib import asynccontextmanager
from pathlib import Path, PurePosixPath
from uuid import uuid4

from harbor.environments.base import BaseEnvironment, ExecResult
from harbor.environments.capabilities import (
    EnvironmentCapabilities,
    EnvironmentResourceCapabilities,
)
from harbor.models.task.config import NetworkMode, NetworkPolicy
from harbor.models.trial.paths import EnvironmentPaths

from dvm import AsyncSandbox, CommandResult

SANDBOX_NAME_LENGTH = 63


def sandbox_name(task: str, role: str) -> str:
    """A dvm.run sandbox name: lowercase letters, digits and '-', 63 at most."""
    suffix = f"-{role}"
    stem = re.sub(r"[^a-z0-9]+", "-", task.lower()).strip("-")
    stem = stem[: SANDBOX_NAME_LENGTH - len(suffix)].rstrip("-") or "task"
    return stem + suffix


class DvmEnvironment(BaseEnvironment):
    """One public prebuilt Linux image, with a complete replayable recording."""

    def __init__(self, *args, **kwargs):
        self._sandbox: AsyncSandbox | None = None
        self._closed = False
        super().__init__(*args, **kwargs)
        if kwargs.get("network_policy") is None:
            self._network_policy = self.task_env_config.resolve_baseline()
            self._validate_network_policy_support()

    @staticmethod
    def type() -> str:
        return "dvm"

    @classmethod
    def resource_capabilities(cls) -> EnvironmentResourceCapabilities:
        return EnvironmentResourceCapabilities(cpu_limit=True, memory_limit=True)

    @property
    def capabilities(self) -> EnvironmentCapabilities:
        return EnvironmentCapabilities(
            disable_internet=True,
            network_allowlist=True,
            network_allowlist_hostnames=True,
            network_allowlist_wildcard_hostnames=True,
            network_allowlist_ipv4_addresses=True,
            network_allowlist_ipv4_cidrs=True,
            dynamic_network_policy=True,
        )

    def _validate_definition(self) -> None:
        compose_names = ("docker-compose.yaml", "docker-compose.yml", "compose.yaml", "compose.yml")
        if any((self.environment_dir / name).exists() for name in compose_names):
            raise ValueError("dvm.run does not support Docker Compose or multiple containers")
        if self._host_mounts:
            raise ValueError("dvm.run does not support host mounts; use file transfers")
        if not self.task_env_config.docker_image:
            raise ValueError(
                "dvm.run requires docker_image naming a public prebuilt image; "
                "it cannot build a Dockerfile. Set [environment].docker_image "
                "and, for a separate grader, [verifier.environment].docker_image."
            )

    @property
    def sandbox_id(self) -> int | None:
        """The execution ID to replay after stop; retained after closing."""
        return self._sandbox.id if self._sandbox is not None else None

    def _running_sandbox(self) -> AsyncSandbox:
        if self._sandbox is None or self._closed:
            raise RuntimeError("dvm.run environment is not running")
        return self._sandbox

    @staticmethod
    def _network_options(policy: NetworkPolicy) -> dict:
        if policy.network_mode == NetworkMode.NO_NETWORK:
            return {"network": "none"}
        if policy.network_mode == NetworkMode.ALLOWLIST:
            return {"network": "allowlist", "allow": list(policy.allowed_hosts)}
        return {"network": "public"}

    async def _apply_network_policy(self, network_policy: NetworkPolicy) -> None:
        if self._sandbox is not None:
            await self._sandbox.set_network(**self._network_options(network_policy))

    async def start(self, force_build: bool = False) -> None:
        if force_build:
            raise ValueError("dvm.run cannot build images; use a public prebuilt docker_image")
        self._validate_definition()
        if self._sandbox is not None:
            raise RuntimeError("dvm.run environment has already started")
        # Harbor has no role argument; separate verifier session IDs carry this marker.
        role = "grading" if "__verifier__" in self.session_id else "agent"
        self._sandbox = await AsyncSandbox.create(
            self.task_env_config.docker_image,
            cpus=self._effective_cpus,
            memory_mib=self._effective_memory_mb,
            env=self._startup_env(),
            name=sandbox_name(self.environment_name, role),
            **self._network_options(self.network_policy),
        )
        try:
            paths = EnvironmentPaths.for_os(self.os)
            directories = [str(path) for path in (
                paths.agent_dir, paths.verifier_dir, paths.artifacts_dir,
                *self._mount_targets(writable_only=True),
            )]
            await self._checked_run(["mkdir", "-p", "--", *directories])
            await self._checked_run(["chmod", "777", "--", *directories])
            await self._upload_environment_dir_after_start()
        except BaseException:
            await self.stop(delete=False)
            raise

    async def stop(self, delete: bool = True) -> None:
        sandbox = self._sandbox
        if sandbox is None or self._closed:
            return
        try:
            try:
                async with asyncio.timeout(30):
                    await sandbox.end()
            except Exception:
                self.logger.warning("Could not end dvm.run input; stopping the sandbox", exc_info=True)
                await sandbox.stop()
        finally:
            try:
                await sandbox.close()
            finally:
                self._closed = True

    async def exec(
        self,
        command: str,
        cwd: str | None = None,
        env: dict[str, str] | None = None,
        timeout_sec: int | float | None = None,
        user: str | int | None = None,
    ) -> ExecResult:
        sandbox = self._running_sandbox()
        resolved_user = self._resolve_user(user)
        options = dict(
            cwd=cwd if cwd is not None else self.task_env_config.workdir,
            env=self._merge_env(env),
            user=str(resolved_user) if resolved_user is not None else None,
        )
        if timeout_sec is None:
            result = await sandbox.run(command, **options)
        else:
            result = await self._run_with_timeout(command, options, timeout_sec)
        return ExecResult(
            stdout=result.stdout.decode("utf-8", errors="replace"),
            stderr=result.stderr.decode("utf-8", errors="replace"),
            return_code=result.exit_code,
        )

    async def _run_with_timeout(self, command: str, options: dict, timeout_sec: float) -> CommandResult:
        if timeout_sec <= 0:
            raise TimeoutError("dvm.run command timeout must be positive")
        sandbox = self._running_sandbox()
        started = asyncio.create_task(sandbox.start(command, stdin=b"", **options))
        try:
            async with asyncio.timeout(timeout_sec):
                handle = await asyncio.shield(started)
                return await handle.wait()
        except (TimeoutError, asyncio.CancelledError):
            # Shield dispatch so a deadline cannot lose the handle to a live command.
            try:
                async with asyncio.timeout(30):
                    handle = await started
                    await handle.signal(signal.SIGKILL)
                    await handle.wait()
            except Exception:
                await self.stop(delete=False)
            raise

    async def _checked_run(self, command: list[str]) -> None:
        result = await self._running_sandbox().run(command, user="root")
        if result.exit_code != 0:
            reason = result.stderr.decode("utf-8", errors="replace")
            raise RuntimeError(f"dvm.run command failed (exit {result.exit_code}): {reason}")

    async def upload_file(self, source_path: Path | str, target_path: str) -> None:
        source = Path(source_path)
        await self._checked_run(["mkdir", "-p", "--", str(PurePosixPath(target_path).parent)])
        await self._running_sandbox().write_file(
            target_path, source.read_bytes(), mode=source.stat().st_mode & 0o777,
        )

    async def download_file(self, source_path: str, target_path: Path | str) -> None:
        data = await self._running_sandbox().read_file(source_path)
        target = Path(target_path)
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(data)

    @asynccontextmanager
    async def _remote_archive(self):
        path = f"/tmp/dvm-harbor-{uuid4().hex}.tar"
        try:
            yield path
        finally:
            await self._checked_run(["rm", "-f", "--", path])

    async def upload_dir(self, source_dir: Path | str, target_dir: str) -> None:
        source = Path(source_dir)
        if not source.is_dir():
            raise FileNotFoundError(f"Source directory does not exist: {source}")
        buffer = io.BytesIO()
        with tarfile.open(fileobj=buffer, mode="w") as archive:
            for child in sorted(source.iterdir()):
                archive.add(child, arcname=child.name)
        async with self._remote_archive() as remote:
            await self._running_sandbox().write_file(remote, buffer.getvalue())
            await self._checked_run(["mkdir", "-p", "--", target_dir])
            await self._checked_run(["tar", "--no-same-owner", "-xf", remote, "-C", target_dir])

    async def download_dir(self, source_dir: str, target_dir: Path | str) -> None:
        async with self._remote_archive() as remote:
            await self._checked_run(["tar", "-cf", remote, "-C", source_dir, "."])
            data = await self._running_sandbox().read_file(remote)
        target = Path(target_dir)
        target.mkdir(parents=True, exist_ok=True)
        with tarfile.open(fileobj=io.BytesIO(data), mode="r:*") as archive:
            for member in archive:
                if member.isdev():
                    continue
                try:
                    archive.extract(member, target, filter="data")
                except tarfile.FilterError as error:
                    self.logger.debug("Skipping unsafe archive member %r: %s", member.name, error)
