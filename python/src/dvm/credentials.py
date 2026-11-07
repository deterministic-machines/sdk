"""The API keys ``dvm-run login`` saved, by the address of the dvm.run each
is for: ``dvm/credentials.json`` in ``$XDG_CONFIG_HOME``, else in
``~/.config``, readable by its user alone."""

from __future__ import annotations

import json
import os
from pathlib import Path
from typing import Optional

from dvm.errors import DvmError


def credentials_path() -> Path:
    config = os.environ.get("XDG_CONFIG_HOME") or Path.home() / ".config"
    return Path(config) / "dvm" / "credentials.json"


def saved_key(api_url: str) -> Optional[str]:
    """The key saved for the dvm.run at ``api_url``, if there's one."""
    return _saved_keys(credentials_path()).get(_address(api_url))


def save_key(api_url: str, key: str) -> Path:
    """Saves ``key`` for the dvm.run at ``api_url``, in place of the one
    saved for it before; the file it's in."""
    path = credentials_path()
    keys = _saved_keys(path)
    keys[_address(api_url)] = key
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    # Written beside it, then moved over it, so a reader never sees half of
    # it; made for its user alone before the key is in it.
    written = path.with_name(path.name + ".new")
    written.unlink(missing_ok=True)
    descriptor = os.open(written, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, "w") as file:
        json.dump({"keys": keys}, file, indent=2)
        file.write("\n")
    os.replace(written, path)
    return path


def _saved_keys(path: Path) -> dict[str, str]:
    try:
        text = path.read_text()
    except FileNotFoundError:
        return {}
    try:
        keys = json.loads(text)["keys"]
        if isinstance(keys, dict) and all(isinstance(key, str) for key in keys.values()):
            return keys
    except (ValueError, KeyError, TypeError):
        pass
    raise DvmError(f"{path} isn't what `dvm-run login` writes: delete it, and log in again")


def _address(api_url: str) -> str:
    return api_url.rstrip("/")
