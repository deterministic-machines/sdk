"""Where a sandbox, or a replay, is."""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any, Mapping, Optional


@dataclass(frozen=True)
class RunState:
    """``queued``, ``running``, ``paused``; then ``exited`` with its
    ``exit_code``, ``stopped``, or ``failed`` with its ``reason``."""

    kind: str
    exit_code: Optional[int] = None
    reason: Optional[str] = None

    @classmethod
    def from_api(cls, state: Mapping[str, Any]) -> RunState:
        return cls(state["kind"], state.get("code"), state.get("reason"))

    @property
    def is_active(self) -> bool:
        """Whether it hasn't ended."""
        return self.kind in ("queued", "running", "paused")
