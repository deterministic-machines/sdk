"""Harbor's oracle, artifact handoff, grading, and both complete replays."""

import asyncio
import os
from pathlib import Path

import pytest

pytest.importorskip("harbor")

from harbor.models.trial.config import TrialConfig
from harbor.trial.trial import Trial

from dvm import AsyncExecution
from dvm.harbor import DvmEnvironment


@pytest.mark.platform
@pytest.mark.skipif(
    not (os.environ.get("DVM_API_URL") and os.environ.get("DVM_API_KEY")),
    reason="needs DVM_API_URL and DVM_API_KEY",
)
def test_oracle_reward_and_both_recordings_replay_same(tmp_path, monkeypatch):
    environments = []
    original_start = DvmEnvironment.start

    async def remember_start(environment, force_build=False):
        await original_start(environment, force_build=force_build)
        environments.append(environment)

    monkeypatch.setattr(DvmEnvironment, "start", remember_start)

    async def run():
        config = TrialConfig(
            task={"path": Path(__file__).parent / "harbor-task"},
            agent={"name": "oracle"},
            environment={"import_path": "dvm.harbor:DvmEnvironment"},
            trials_dir=tmp_path,
        )
        trial = await Trial.create(config)
        result = await trial.run()
        assert result.exception_info is None, result.exception_info
        assert result.verifier_result.rewards == {"reward": 1.0}
        assert len(environments) == 2
        names = set()
        for environment in environments:
            execution = await AsyncExecution.get(environment.sandbox_id)
            try:
                names.add(execution.spec["name"])
                assert execution.state.kind == "exited"
                assert (await (await execution.replay()).wait()).same
            finally:
                await execution.close()
        assert names == {"harbor-replay-agent", "harbor-replay-grading"}

    asyncio.run(run())
