# dvm-run

dvm.run's Python SDK.

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

A sandbox records its process from start to end. With no command or
entrypoint it runs the sandbox server and takes commands; the image's own
command does not run. Commands inherit the image's environment, user and
working directory, with any environment you supply. With a command or
entrypoint it is one run of that program, and command and file methods
raise `DvmError` without sending a request.

```python
from dvm import Sandbox, Execution

with Sandbox.create("python:3.12") as sandbox:
    result = sandbox.run(["python", "-c", "print(2)"])
    assert result.exit_code == 0 and result.stdout == b"2\n"
    sandbox.write_file("/tmp/a.txt", b"hi")
    assert sandbox.read_file("/tmp/a.txt") == b"hi"
    server = sandbox.start("python -m http.server 8000")
    server.signal(15)
    server.wait()
assert Execution.get(sandbox.id).replay().wait().same
```

`run(command, *, env=None, cwd=None, stdin=None)` runs argv (a list) or a
shell script (a string) to its end. It sends stdin bytes, then closes stdin;
without bytes, stdin is empty. Its `CommandResult` has `exit_code`, `stdout`
and `stderr`, the last two as bytes. A nonzero code is a result, not an
error. A killing signal is reported as `128 + signal`; the protocol does
not distinguish that from a program explicitly exiting with that code.

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
exit, so the recording replays. `end()` does this explicitly and returns
the sandbox's exit code. EOF makes the server kill any commands still
running and exit 0. A paused sandbox cannot read EOF, so scope end stops
it instead. A sandbox with a supplied program is also stopped at scope
end if still active. A stopped recording does not replay.

- Making a sandbox returns once its process started; one already exited
  is fine. `wait()` gives that process's exit code, including nonzero.
  A stopped sandbox raises `SandboxEndedError`; a failed sandbox raises
  `SandboxFailedError` with dvm's reason. Pending operations raise these
  errors too if the sandbox ends before answering.
- `output()` returns raw process output. For command sandboxes, `stdout`
  is the server's JSON answer lines. For a supplied program, dvm combines
  its stdout and stderr in `stdout`; `stderr` is dvm's own diagnostics.
- `pause`, `resume` and `stop` act on an active sandbox. A replay's verdict
  is `same`, `other_exit` (both codes) or `other_output` (the stream and the
  first byte that differs).
- `AsyncSandbox`, `AsyncCommand`, `AsyncExecution` and `AsyncReplay` have
  the same methods for asyncio; blocking classes use those implementations.
- The address is `DVM_API_URL`, the key is `DVM_API_KEY`, else the key
  `dvm-run login` saved for that address; both can be given in code.
