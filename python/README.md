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

A sandbox is one recorded run of one program: the image's own command, or
the command and entrypoint you give as you make it. It ends when the program
exits. Its execution replays, the same, once the program exited.

```python
from dvm import Sandbox, Execution

with Sandbox.create("python:3.12", command=["python", "-c", "print(1 + 1)"], seed=7) as sandbox:
    print(sandbox.wait())
    print(sandbox.output().stdout.decode())

execution = Execution.get(sandbox.id)
replay = execution.replay()
assert replay.wait().same
print(replay.output().stdout.decode())
execution.close()
```

- Making a sandbox returns once its program started; one that already
  exited is fine.
- The program's exit code is never an error. `wait()` raises `SandboxEndedError` for a sandbox that
  was stopped, and `SandboxFailedError`, with dvm's reason, for one that
  failed.
- Output is bytes. In dvm a program's stdout and stderr are one stream, in
  the order it wrote them: `stdout`. `stderr` is what dvm itself said.
- `pause`, `resume` and `stop` act on a sandbox that is going. A stopped
  sandbox's log is cut short, and doesn't replay.
- A replay's verdict is `same`, `other_exit` (both codes) or `other_output`
  (the stream, and the first byte that differs).
- As a context manager, a sandbox still going is stopped at the end.
  `AsyncSandbox`, `AsyncExecution` and `AsyncReplay` are the same API for
  asyncio.
- It takes dvm.run's address from `DVM_API_URL`, and its key from
  `DVM_API_KEY`, else the one `dvm-run login` saved for that address; both
  may be given in code instead.
