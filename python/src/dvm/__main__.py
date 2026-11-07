"""``dvm-run``, the SDK's command line, also ``python -m dvm``."""

from __future__ import annotations

import argparse
import sys
from typing import Optional, Sequence

import httpx

from dvm.connection import DEFAULT_API_URL
from dvm.errors import DvmError
from dvm.login import login


def main(argv: Optional[Sequence[str]] = None) -> int:
    parser = argparse.ArgumentParser(prog="dvm-run", description="dvm.run's command line.")
    commands = parser.add_subparsers(dest="command", required=True)
    login_command = commands.add_parser(
        "login",
        help="get an API key for this machine, approved in your browser",
        description="Get an API key for this machine, approved in your browser, "
        "and save it where the SDK finds it.",
    )
    login_command.add_argument(
        "--api-url",
        help=f"the dvm.run to log in to: DVM_API_URL, else {DEFAULT_API_URL}",
    )
    login_command.add_argument(
        "--no-browser",
        action="store_true",
        help="only print the link to approve the login at",
    )
    arguments = parser.parse_args(argv)
    try:
        login(arguments.api_url, open_browser=not arguments.no_browser)
    except DvmError as error:
        print(f"dvm-run login: {error}", file=sys.stderr)
        return 1
    except httpx.HTTPError as error:
        print(f"dvm-run login: couldn't reach dvm.run: {error}", file=sys.stderr)
        return 1
    except KeyboardInterrupt:
        return 130
    return 0


if __name__ == "__main__":
    sys.exit(main())
