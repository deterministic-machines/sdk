# dvm-run

dvm.run's Python SDK, for Python 3.12 and newer.

```sh
pip install dvm-run
dvm-run login
```

`dvm-run login` opens dvm.run in your browser, where you approve an API key
for this machine; it saves the key in `~/.config/dvm/credentials.json`, where
the SDK finds it. Over SSH, open the link it prints wherever you like. Where
no one is there to approve one, such as CI, make a key on
[dvm.run's API keys page](https://sandbox.dvm.run/keys) and set
`DVM_API_KEY`.

A sandbox captures its execution from start to end so you can seek back
and inspect its state later. With no command or entrypoint it runs the
sandbox server and takes commands; the image's own command does not run.
Commands run as root, in the image's environment and working directory,
with any environment you supply; a command's `user` runs it as another of
the image's users. With a command or entrypoint it is one run of that
program, and command and file methods raise `DvmError` without sending a
request.

```python
from dvm import Sandbox

with Sandbox.create("python:3.12") as sandbox:
    result = sandbox.run(["python", "-c", "print(2)"])
    assert result.exit_code == 0 and result.stdout == b"2\n"
    sandbox.write_file("/tmp/a.txt", b"hi")
    assert sandbox.read_file("/tmp/a.txt") == b"hi"
    server = sandbox.start("python -m http.server 8000")
    server.signal(15)
    server.wait()
    sandbox.end()
    point = sandbox.seek()
    assert point.read_file("/tmp/a.txt") == b"hi"
```

`run(command, *, env=None, cwd=None, stdin=None, user=None)` runs argv (a
list) or a shell script (a string) to its end. It sends stdin bytes, then
closes stdin; without bytes, stdin is empty. Its `CommandResult` has
`exit_code`, `stdout` and `stderr`, the last two as bytes. A nonzero code is
a result, not an error. A killing signal is reported as `128 + signal`; the
protocol does not distinguish that from a program explicitly exiting with
that code.

`user` is named as `docker exec --user` names one: a name or uid, optionally
`:group` or `:gid`, looked up in the image's `/etc/passwd` and `/etc/group`.
`HOME` and `USER` are that user's unless you supply them. An unknown user
exits 126 with the reason on stderr.

A sandbox reaches the internet unless made otherwise: `network="none"`
reaches nothing, not even a resolver, and `allow=["pypi.org",
"*.github.com", "8.8.8.0/24"]` makes an allowlist: the resolver, the
internet addresses it gives those names, and those addresses. It never
reaches its runner, the runner's LAN, or link-local addresses.
`set_network(network, allow=None)` changes it while the sandbox runs; open
connections stay. A refused connection fails with "Network unreachable".

`start` takes the same arguments and returns a `Command` once started.
`wait()` gives its result; `signal(number)` signals its process group.
With omitted stdin, the handle keeps stdin open: `write_stdin(bytes)`
sends pieces and `close_stdin()` closes it. Supplied stdin is sent and
closed before `start` returns. Stdin and signal success means the request
was sent; the server sends no acknowledgment. Descendants may write after
the command exits; the result contains output through the command's exit.

`write_file(path, data, *, mode=None)` and `read_file(path)` move whole
files as bytes, in pieces of any total size within the sandbox's limits.
Write mode defaults to `0o644`, subject to umask; an existing file keeps
its mode. Parent directories must exist. Writes are not atomic. A write
that fails part-way leaves the file partly written; the protocol has no
way to take it back.

Use only one writer per sandbox. Two handles writing at once can corrupt
its input; `connect(id)` learns the input length but does not lock out
another writer. A server refusal raises `DvmError` with its message. Input
flow control can wait up to 30 seconds, then refuse if the program is not
reading; the SDK passes that error on without retrying.

At scope end a command sandbox's input is ended and the SDK waits for its
exit, leaving it ready to inspect. `end()` does this explicitly and
returns the sandbox's exit code. EOF makes the server kill any commands
still running and exit 0. A paused sandbox cannot read EOF, so scope end
stops it instead. A sandbox with a supplied program is also stopped at
scope end if still active. A stopped sandbox cannot be sought.

- Making a sandbox returns once its process started; one already exited
  is fine. `wait()` gives that process's exit code, including nonzero.
  A stopped sandbox raises `SandboxEndedError`; a failed sandbox raises
  `SandboxFailedError` with dvm's reason. Pending operations raise these
  errors too if the sandbox ends before answering.
- `output()` returns raw process output. For command sandboxes, `stdout`
  is the server's answer frames. For a supplied program, dvm combines
  its stdout and stderr in `stdout`; `stderr` is dvm's own diagnostics.
- `pause`, `resume` and `stop` act on an active sandbox.
- `seek(at_ns=None, thread=None, event=None, watch=None)` returns a
  `SandboxPoint` with metadata (`ns`, `thread`, `process`, `operations`,
  `event`, `at`, `stats`), `step(watch=None)` (returning `None` after the
  last event), `stat(path, follow_symlinks=True)`, `list_dir(path)`, and
  `read_file(path)`. Resources are released automatically when idle and
  restored transparently if queried again.
- `AsyncSandbox` and `AsyncSandboxPoint` have the same methods for
  asyncio; blocking classes use those implementations.
- The address is `DVM_API_URL`, the key is `DVM_API_KEY`, else the key
  `dvm-run login` saved for that address; both can be given in code.

## Harbor and Terminal-Bench

Install the SDK in Harbor's own environment, then select the provider:

```sh
uv tool install harbor --with dvm-run
harbor run -p <task dir> --agent oracle --env dvm.harbor:DvmEnvironment
```

Harbor is not a dependency of `dvm-run`, and there is no Harbor extra.
Only `dvm.harbor` imports Harbor; Harbor loads that module from its own
installation. The provider is tested with Harbor 0.26.0. Credentials are
the SDK's usual `DVM_API_URL`, `DVM_API_KEY`, or a saved `dvm-run login`.

Each environment becomes a command sandbox from its `docker_image`, with
the task's whole CPU count, memory and network policy. A command honors
Harbor's working directory, environment and user. At a command deadline,
the provider kills its process group and raises `TimeoutError`. Files use
the SDK's file operations; directories travel as tar archives, preserving
executable bits, links within the directory and empty directories. Images
must include a POSIX shell and `tar`.

The provider supports no network, public internet, and allowlists of exact
hostnames, `*.domain` names, IPv4 addresses and IPv4 networks. Harbor can
change the policy between phases through `set_network`. IPv6 is not
supported. dvm.run always excludes the runner, its LAN and link-local
addresses; existing connections survive a policy change.

Harbor chooses shared or separate grading from the task configuration.
For separate grading it moves the declared artifacts between sandboxes.
The provider names them `<task>-agent` and `<task>-grading`, cut to
dvm.run's rule for names (63 lowercase letters, digits and `-`). Harbor has
no role parameter, so the provider identifies grading by `__verifier__`
in Harbor's `session_id`. Stop ends input and waits for exit, keeping the
completed sandbox inspectable; it stops the sandbox only if ending input
fails or takes more than 30 seconds. Both values of Harbor's `delete`
argument retain the sandbox history. `sandbox_id` gives its ID.

dvm.run runs public prebuilt Linux images. The provider refuses missing
images, image builds (including `force_build`), Docker Compose, multiple
containers, GPUs, TPUs, Windows and host mounts before creating a sandbox.
A Dockerfile alongside a `docker_image` is not built. Storage uses the
platform's default; Harbor's `storage_mb` is not applied.

The published `terminal-bench/terminal-bench@latest` registry was checked
on 2026-11-14: all 66 tasks have digest-pinned `docker_image` values under
both `[environment]` and `[verifier.environment]`, and separate graders.
The dataset digest was
`sha256:5881c1b1c74a96f07baef00a7c869e09802704bc0dc912c870e7acccc11440da`.
Download those tasks with:

```sh
harbor download terminal-bench/terminal-bench@latest -o tasks
```

Source checkouts can omit these image fields. To use such a checkout,
name the already published images in `task.toml`: set
`[environment].docker_image` to the release manifest's environment
`pinned_ref`, and `[verifier.environment].docker_image` to its verifier
`pinned_ref`, with `[verifier].environment_mode = "separate"`. An explicit
verifier image must already contain its tests; Harbor treats them as
bundled. Tasks requiring Compose or accelerators remain unsupported.

## Tests

From the repository root, the Nix shell supplies Python, pytest and uv:

```sh
nix develop -c bash -c 'cd python && pytest -q'
nix develop -c bash -c 'cd python && uv run --no-project --with harbor --with pytest --with-editable . pytest -q'
```

The first command skips the Harbor modules when Harbor is absent. The
second runs them in a uv-managed environment without adding Harbor to the
SDK's dependencies. To run only the oracle and inspection test, set
`DVM_API_URL` and `DVM_API_KEY`, then run:

```sh
nix develop -c bash -c 'cd python && uv run --no-project --with harbor --with pytest --with-editable . pytest -q tests/test_harbor_platform.py'
```

That test uses `tests/harbor-task`, a tiny task on `python:3.12-slim`. It
checks the oracle reward, the artifact handoff to separate grading, and
that both completed sandboxes can be sought and inspected.

## Wire format

Input bodies are raw bytes; output bodies and sandbox protocol messages
use JSON headers followed by counted raw bytes. Stdin and file writes
use payload pieces of at most 1 MiB. The SDK splits input across 64 KiB
HTTP posts and follows output pages, advancing stdout and stderr offsets
until `more` is false. See the [shared wire format](../README.md#bytes-on-the-wire).
