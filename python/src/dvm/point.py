"""Output streams and point-in-time sandbox inspection."""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from typing import Any, Optional

from dvm.connection import Connection
from dvm.frame import decode_output

_NAMED_WATCH_POINTS: dict[str, dict[str, Any]] = {
    "syscall": {"kind": "syscall"},
    "page-fault": {"kind": "page_fault"},
    "page_fault": {"kind": "page_fault"},
    "processes": {"kind": "processes"},
    "file-syscalls": {"kind": "file_syscalls"},
    "file_syscalls": {"kind": "file_syscalls"},
    "socket-syscalls": {"kind": "socket_syscalls"},
    "socket_syscalls": {"kind": "socket_syscalls"},
}


@dataclass(frozen=True)
class Output:
    """What the process wrote: raw bytes on stdout and stderr."""

    stdout: bytes
    stderr: bytes


async def read_output(connection: Connection, path: str) -> Output:
    """Pages through a run's output from byte 0 until a page that is not cut."""
    stdout = bytearray()
    stderr = bytearray()
    params: Optional[dict[str, int]] = None
    while True:
        page = decode_output(await connection.bytes(path, params=params))
        stdout.extend(page.stdout)
        stderr.extend(page.stderr)
        if not page.cut:
            return Output(bytes(stdout), bytes(stderr))
        params = {"offset": len(stdout), "stderr_offset": len(stderr)}


@dataclass(frozen=True)
class DecodedArgument:
    arg: int
    kind: str
    value: Any
    truncated: bool = False

    @classmethod
    def from_api(cls, payload: Mapping[str, Any]) -> DecodedArgument:
        for key in ("path", "strings", "address", "bytes"):
            if key in payload:
                return cls(
                    arg=int(payload["arg"]),
                    kind=key,
                    value=payload[key],
                    truncated=bool(payload.get("truncated", False)),
                )
        raise ValueError(f"unexpected decoded argument shape: {payload!r}")


@dataclass(frozen=True)
class StopEvent:
    kind: str
    syscall: Optional[int] = None
    name: Optional[str] = None
    args: tuple[str, ...] = ()
    decoded: tuple[DecodedArgument, ...] = ()
    address: Optional[str] = None
    write: Optional[bool] = None
    present: Optional[bool] = None

    @classmethod
    def from_api(cls, payload: Mapping[str, Any]) -> StopEvent:
        if "syscall" in payload:
            syscall = payload["syscall"]
            return cls(
                kind="syscall",
                syscall=int(syscall["number"]),
                name=str(syscall["name"]),
                args=tuple(syscall.get("args", ())),
                decoded=tuple(
                    DecodedArgument.from_api(item) for item in syscall.get("decoded", ())
                ),
            )
        if "page_fault" in payload:
            fault = payload["page_fault"]
            return cls(
                kind="page_fault",
                address=str(fault["address"]),
                write=bool(fault["write"]),
                present=bool(fault["present"]),
            )
        raise ValueError(f"unexpected stop event shape: {payload!r}")


@dataclass(frozen=True)
class DirEntry:
    name: str
    raw_name: bytes
    type: str

    @classmethod
    def from_api(cls, payload: Mapping[str, Any]) -> DirEntry:
        return cls(
            name=str(payload["name"]),
            raw_name=bytes.fromhex(str(payload["raw_name"])),
            type=str(payload["type"]),
        )


@dataclass(frozen=True)
class FileStat:
    type: str
    mode: int
    uid: int
    gid: int
    size: int
    links: int
    mtime: tuple[int, int]
    ctime: tuple[int, int]
    target: Optional[bytes] = None
    mount: Optional[str] = None

    @classmethod
    def from_api(cls, payload: Mapping[str, Any]) -> FileStat:
        raw_target = payload.get("target")
        mtime = payload["mtime"]
        ctime = payload["ctime"]
        return cls(
            type=str(payload["type"]),
            mode=int(payload["mode"]),
            uid=int(payload["uid"]),
            gid=int(payload["gid"]),
            size=int(payload["size"]),
            links=int(payload["links"]),
            mtime=(int(mtime[0]), int(mtime[1])),
            ctime=(int(ctime[0]), int(ctime[1])),
            target=bytes.fromhex(str(raw_target)) if raw_target is not None else None,
            mount=payload.get("mount"),
        )


class AsyncSandboxPoint:
    """One point in a sandbox's execution history, with file inspection and stepping."""

    def __init__(
        self,
        connection: Connection,
        sandbox_id: int,
        stop: Mapping[str, Any],
    ) -> None:
        self._connection = connection
        self.sandbox_id = sandbox_id
        self.thread = int(stop["thread"])
        self.process = int(stop["process"])
        self.operations = int(stop["operations"])
        self.event = int(stop["event"])
        raw_ns = stop.get("recording_ns")
        self.ns: Optional[int] = int(raw_ns) if raw_ns is not None else None
        self.at = StopEvent.from_api(stop["at"])
        self.stats: dict[str, int] = {
            str(key): int(value) for key, value in stop.get("stats", {}).items()
        }

    async def step(
        self, *, watch: Optional[Sequence[Any]] = None
    ) -> Optional[AsyncSandboxPoint]:
        body = {"thread": self.thread, "event": self.event, **_encode_watches(watch)}
        stop = await self._connection.json(
            "POST", f"/sandboxes/{self.sandbox_id}/points/step", body=body, idempotent=True,
        )
        return AsyncSandboxPoint(self._connection, self.sandbox_id, stop) if stop is not None else None

    async def read_file(self, path: str) -> bytes:
        return await self._connection.bytes(
            f"/sandboxes/{self.sandbox_id}/fs/read",
            params=self._fs_params(path),
        )

    async def list_dir(self, path: str) -> list[DirEntry]:
        entries = await self._connection.json(
            "GET",
            f"/sandboxes/{self.sandbox_id}/fs/list",
            params=self._fs_params(path),
        )
        return [DirEntry.from_api(entry) for entry in entries]

    async def stat(self, path: str, *, follow_symlinks: bool = True) -> FileStat:
        status = await self._connection.json(
            "GET",
            f"/sandboxes/{self.sandbox_id}/fs/stat",
            params={
                **self._fs_params(path),
                "follow_symlinks": "true" if follow_symlinks else "false",
            },
        )
        return FileStat.from_api(status)

    def _fs_params(self, path: str) -> dict[str, Any]:
        return {"thread": self.thread, "event": self.event, "path": path}


async def seek_point(
    connection: Connection,
    sandbox_id: int,
    target: Mapping[str, Any],
    watch: Optional[Sequence[Any]] = None,
) -> AsyncSandboxPoint:
    body = {"target": dict(target), **_encode_watches(watch)}
    stop = await connection.json(
        "POST", f"/sandboxes/{sandbox_id}/points/seek", body=body, idempotent=True,
    )
    return AsyncSandboxPoint(connection, sandbox_id, stop)


def _encode_watches(watch: Optional[Sequence[Any]]) -> dict[str, Any]:
    if watch is None:
        return {}
    encoded: list[dict[str, Any]] = []
    for item in watch:
        if isinstance(item, Mapping):
            encoded.append(dict(item))
        elif isinstance(item, str):
            if item in _NAMED_WATCH_POINTS:
                encoded.append(dict(_NAMED_WATCH_POINTS[item]))
            elif item.startswith("syscall:"):
                encoded.append({"kind": "named_syscall", "name": item[len("syscall:") :]})
            else:
                raise ValueError(f"unknown watch point: {item!r}")
        else:
            raise TypeError(f"watch point must be a string or mapping, got {type(item).__name__}")
    return {"watch_points": encoded}
