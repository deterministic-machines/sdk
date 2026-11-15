#!/bin/sh
set -eu
mkdir -p /logs/verifier
python - <<'PY'
from pathlib import Path

reward = int(Path("/app/answer.txt").read_text() == "42\n")
Path("/logs/verifier/reward.txt").write_text(str(reward))
PY
