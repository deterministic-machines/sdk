# dvm-run

dvm.run's JavaScript SDK, for Node 22 and later.

```sh
npm install dvm-run
npx dvm-run login
```

`dvm-run login` opens dvm.run in your browser, where you approve an API key
for this machine; it saves the key in `~/.config/dvm/credentials.json`, where
the SDK finds it (the Python SDK too). Over SSH, open the link it prints
wherever you like. Where no one is there to approve one, such as CI, make a
key on [dvm.run's API keys page](https://sandbox.dvm.run/keys) and set
`DVM_API_KEY`.

A sandbox is a VM made from an image: your agent sends it commands and
files until it stops it. Everything it ran is its execution, which
replays, the same, once it ends.

```ts
import { Sandbox, Execution } from "dvm-run";

let id: number;
{
  await using sbx = await Sandbox.create("python:3.12", { seed: 7 });
  id = sbx.id;
  await sbx.files.write("/app/main.py", "print(1 + 1)\n");
  const r = await sbx.commands.run("python main.py", { cwd: "/app", timeoutMs: 60_000 });
  console.log(r.stdout, r.exitCode);

  const server = await sbx.commands.run(["python", "-m", "http.server"], { background: true });
  await server.kill();
  await server.wait();
}

const verdict = await (await (await Execution.get(id)).replay()).wait();
console.assert(verdict.same);
```

It takes dvm.run's address as `apiUrl`, else `DVM_API_URL`, and its key
as `apiKey`, else `DVM_API_KEY`, else the one `dvm-run login` saved for
that address.

- A string runs in the sandbox's shell (`/bin/bash -lc`, or `/bin/sh -c`
  in an image without bash); an array runs as it is.
- A command's exit code is never an exception. A sandbox that ended throws
  `SandboxEndedError` (`SandboxFailedError`, with dvm's reason, if it
  failed) on the next call.
- `timeoutMs` kills the command once it passes; its result says so, with
  code 137 and signal 9.
- Every call takes `signal`, an `AbortSignal`. A command it aborts is
  killed, unless it runs in the background: there, it stops only the
  wait.
- `await using` stops the sandbox at the scope's end, once its log is in,
  as `stop()` does. Node 22 has no `await using`: call `stop()`, or let
  TypeScript compile it.
