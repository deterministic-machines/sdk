"""Files in a sandbox, written in and read out whole."""

from __future__ import annotations

from typing import TYPE_CHECKING, Optional, Union

if TYPE_CHECKING:
    from dvm.sandbox import AsyncSandbox


class AsyncFiles:
    """A sandbox's files: ``sandbox.files``."""

    def __init__(self, sandbox: AsyncSandbox) -> None:
        self._sandbox = sandbox

    async def write(self, path: str, data: Union[bytes, str], *, mode: Optional[int] = None) -> None:
        """Writes ``data`` to the file at ``path``, in place of what it held;
        a new file gets ``mode`` (``0o755``), else ``0o644``."""
        params = {"path": path}
        if mode is not None:
            params["mode"] = format(mode, "o")
        content = data.encode() if isinstance(data, str) else data
        sandbox = self._sandbox
        await sandbox._while_live(
            sandbox._connection.bytes("PUT", f"{sandbox._path}/files", params=params, content=content)
        )

    async def read(self, path: str) -> bytes:
        """The file at ``path``, as it is."""
        sandbox = self._sandbox
        return await sandbox._while_live(
            sandbox._connection.bytes("GET", f"{sandbox._path}/files", params={"path": path})
        )

    async def read_text(self, path: str, encoding: str = "utf-8") -> str:
        """The file at ``path``, as text."""
        return (await self.read(path)).decode(encoding)
