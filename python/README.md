# dvm-run

dvm.run's Python SDK: `pip install dvm-run`.

A sandbox is a VM made from an image: your agent sends it commands and
files until it stops it. Everything it ran is its execution, which
replays, the same, once it ends.

```python
from dvm import Sandbox, Execution

with Sandbox.create("python:3.12", seed=7) as sbx:
    sbx.files.write("/app/main.py", "print(1 + 1)\n")
    r = sbx.commands.run("python main.py", cwd="/app", timeout=60)
    print(r.stdout, r.exit_code)

    server = sbx.commands.run(["python", "-m", "http.server"], background=True)
    server.kill()
    server.wait()

verdict = Execution.get(sbx.id).replay().wait()
assert verdict.same
```

It takes its key as `api_key=`, else `DVM_API_KEY`, and dvm.run's address
as `api_url=`, else `DVM_API_URL`. `AsyncSandbox` and `AsyncExecution` are
the same API for asyncio.

- A string runs in the sandbox's shell (`/bin/bash -lc`, or `/bin/sh -c`
  in an image without bash); a list runs as it is.
- A command's exit code is never an exception. A sandbox that ended raises
  `SandboxEndedError` (`SandboxFailedError`, with dvm's reason, if it
  failed) on the next call.
- `timeout=` kills the command once it passes; its result says so, with
  code 137 and signal 9.
