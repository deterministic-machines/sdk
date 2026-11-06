import os

import pytest

from dvm import Sandbox

# dvm-platform's tests run these against a stand-in sandbox, which runs
# commands on the host from a directory of its own, so paths in these are
# relative to it, as they are to alpine's working directory, `/`.
IMAGE = "alpine:3.20"


def pytest_configure(config):
    config.addinivalue_line("markers", "platform: needs DVM_API_URL and DVM_API_KEY")


def pytest_collection_modifyitems(config, items):
    if os.environ.get("DVM_API_KEY"):
        return
    skip = pytest.mark.skip(
        reason="needs dvm.run, or a controller and a runner: DVM_API_URL and DVM_API_KEY"
    )
    for item in items:
        if "platform" in item.keywords:
            item.add_marker(skip)


@pytest.fixture
def sandbox():
    with Sandbox.create(IMAGE) as sandbox:
        yield sandbox
