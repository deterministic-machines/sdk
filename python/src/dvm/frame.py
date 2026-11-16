"""Frames, as in dvm-api's frame.rs: a JSON object on one line, then the
raw bytes it counts in "bytes"."""

from __future__ import annotations

import json
from dataclasses import dataclass
from typing import Any

MAX_HEADER = 64 * 1024
MAX_PAYLOAD = 16 * 1024 * 1024


class FrameError(ValueError):
    pass


class LongHeader(FrameError):
    def __init__(self) -> None:
        super().__init__(f"a frame's header is longer than {MAX_HEADER} bytes")


class LongPayload(FrameError):
    def __init__(self, length: int) -> None:
        self.length = length
        super().__init__(f"a frame counts {length} bytes, more than {MAX_PAYLOAD}")


class BadHeader(FrameError):
    def __init__(self, reason: str) -> None:
        super().__init__(f"a frame's header: {reason}")


class Cut(FrameError):
    def __init__(self) -> None:
        super().__init__("the frame ends early")


class Trailing(FrameError):
    def __init__(self, length: int) -> None:
        self.length = length
        super().__init__(f"{length} bytes follow the frame")


@dataclass(frozen=True)
class Frame:
    header: dict[str, Any]
    payload: bytes
    span: range


def encode_frame(header: dict[str, Any], payload: bytes = b"") -> bytes:
    if not isinstance(header, dict):
        raise TypeError("a frame's header is a JSON object")
    if payload:
        header = {**header, "bytes": len(payload)}
    line = json.dumps(header, ensure_ascii=False, separators=(",", ":"), allow_nan=False)
    return line.encode("utf-8") + b"\n" + payload


class FrameDecoder:
    """Splits a stream, pushed as it comes, into frames. A bad header raises
    BadHeader for its frame alone; a frame too long to hold ends the stream,
    and every later call raises the same error."""

    def __init__(self) -> None:
        self._pending = bytearray()
        self._start = 0
        self._offset = 0
        self._ended: FrameError | None = None

    def push(self, data: bytes) -> None:
        if self._start > len(self._pending) // 2:
            del self._pending[:self._start]
            self._offset += self._start
            self._start = 0
        self._pending.extend(data)

    def buffered(self) -> int:
        return len(self._pending) - self._start

    def next_frame(self) -> Frame | None:
        if self._ended is not None:
            raise self._ended
        newline = self._pending.find(b"\n", self._start, self._start + MAX_HEADER + 1)
        if newline < 0:
            if self.buffered() > MAX_HEADER:
                self._ended = LongHeader()
                raise self._ended
            return None
        length = newline + 1 - self._start
        try:
            header = json.loads(self._pending[self._start:newline])
        except ValueError as error:
            self._take(length)
            raise BadHeader(str(error)) from error
        count = header.pop("bytes", 0) if isinstance(header, dict) else None
        if type(count) is not int or count < 0:
            self._take(length)
            raise BadHeader(f"not an object counting its bytes: {header!r}")
        if count > MAX_PAYLOAD:
            self._ended = LongPayload(count)
            raise self._ended
        if self.buffered() < length + count:
            return None
        payload = bytes(self._pending[newline + 1:newline + 1 + count])
        return Frame(header, payload, self._take(length + count))

    def _take(self, length: int) -> range:
        start = self._offset + self._start
        self._start += length
        return range(start, start + length)


def decode_frame(data: bytes) -> Frame:
    """`data`, whole, as one frame."""
    decoder = FrameDecoder()
    decoder.push(data)
    frame = decoder.next_frame()
    if frame is None:
        raise Cut()
    if decoder.buffered():
        raise Trailing(decoder.buffered())
    return frame


@dataclass(frozen=True)
class OutputAfter:
    """The output route's frame: what came of each stream; whether the run
    may write more; and whether the answer stopped short of what it wrote."""

    stdout: bytes
    stderr: bytes
    more: bool
    cut: bool


def decode_output(data: bytes) -> OutputAfter:
    frame = decode_frame(data)
    header = frame.header
    stdout, stderr = header.get("stdout"), header.get("stderr")
    if (set(header) != {"stdout", "stderr", "more", "cut"}
            or type(stdout) is not int or type(stderr) is not int or stdout < 0 or stderr < 0
            or stdout + stderr != len(frame.payload)
            or type(header["more"]) is not bool or type(header["cut"]) is not bool):
        raise BadHeader(f"output {header!r} in a frame of {len(frame.payload)} bytes")
    return OutputAfter(frame.payload[:stdout], frame.payload[stdout:], header["more"], header["cut"])
