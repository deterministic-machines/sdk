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

A sandbox records its process from start to end. With no command or
entrypoint it runs the sandbox server and takes commands; the image's own
command does not run. Commands inherit the image's environment, user and
working directory, with any environment you supply. With a command or
entrypoint it is one run of that program, and the new command and file
methods fail locally without sending a request.

```ts
import { Sandbox, Execution } from "dvm-run";

let id: number;
{
  await using sandbox = await Sandbox.create("python:3.12");
  id = sandbox.id;
  const result = await sandbox.run(["python", "-c", "print(2)"]);
  console.assert(result.exitCode === 0);
  console.assert(new TextDecoder().decode(result.stdout) === "2\n");
  await sandbox.writeFile("/tmp/a.txt", new TextEncoder().encode("hi"));
  console.assert(new TextDecoder().decode(await sandbox.readFile("/tmp/a.txt")) === "hi");
  const server = await sandbox.start("python -m http.server 8000");
  await server.signal(15);
  await server.wait();
}
console.assert((await (await (await Execution.get(id)).replay()).wait()).same);
```

`run(command, { env, cwd, stdin }?)` takes argv (a string array) or a shell
script (a string), optional environment and working directory, and stdin
as `Uint8Array`. It sends stdin and closes it, then returns `CommandResult`:
`exitCode`, `stdout`, and `stderr`, with both streams as `Uint8Array`.

`start` takes the same arguments and returns `Command` once started.
`wait()` gives its result; `signal(number)` signals its process group.
With omitted stdin the handle keeps stdin open: `writeStdin(bytes)` sends
pieces and `closeStdin()` closes it. Supplied stdin is sent and closed
before `start` returns. Successful stdin and signal calls mean the request
was sent; the server sends no acknowledgment. Descendants may write after
the command exits; the result contains output through the command's exit.

`writeFile(path, data, { mode }?)` and `readFile(path)` move whole files as
bytes, in pieces of any total size within the sandbox's limits. Mode
defaults to `0o644`, subject to umask; existing files keep their mode.
Parent directories must exist. Writes are not atomic. A write that fails
part-way leaves the file partly written; the protocol has no way to take
it back.

A nonzero command code is a result, not an error. A killing signal is
reported as `128 + signal`; the protocol cannot distinguish that from
an explicit exit with that code. Server refusals throw `DvmError` with
their message. Input flow control may wait up to 30 seconds, then refuse
if the program is not reading; the SDK passes the error on without retrying.

Use only one writer per sandbox. Two handles writing at once can corrupt
its input. `connect(id)` learns the length but does not lock out another
writer. JavaScript ids and offsets must stay within safe integers.

At scope end, `await using` ends a command sandbox's input and waits for
exit, so its recording replays. `end()` does this explicitly and returns
the sandbox's exit code. EOF kills any commands still running and makes
the server exit 0. A paused sandbox cannot read EOF, so scope end stops
it instead. A supplied program is also stopped if still active. A stopped
recording does not replay. On Node 22, which cannot parse `await using`,
use `try/finally` and `await sandbox[Symbol.asyncDispose]()` in the finally,
or compile the TypeScript example.

- Making a sandbox returns once its process started; one already exited
  is fine. `wait()` gives that process's exit code. Stopped and failed
  sandboxes throw `SandboxEndedError` and `SandboxFailedError`; pending
  operations throw the same errors if the sandbox ends before answering.
- `output()` is raw process output: a command sandbox's stdout is the
  server's JSON answer lines. A supplied program's stdout and stderr are
  merged into `stdout` by dvm; `stderr` is dvm's own diagnostics.
- `pause`, `resume`, `stop`, `state`, and `connect` act as before. A replay
  verdict is `same`, `other_exit` (both codes), or `other_output` (stream
  and first differing byte).
- Calls and command waits take `{ signal }` for an `AbortSignal`. Aborting
  a wait leaves the command running.
- The address is `DVM_API_URL`, the key is `DVM_API_KEY`, else the key
  `dvm-run login` saved for that address; both can be given in code.
