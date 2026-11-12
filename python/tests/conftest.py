import os

import pytest


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
