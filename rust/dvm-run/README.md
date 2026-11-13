# dvm-run

dvm.run's Rust SDK, on tokio.

```sh
cargo add dvm-run
cargo install dvm-run && dvm-run login
```

`dvm-run login` opens dvm.run in your browser, where you approve an API key
for this machine; it saves the key in `~/.config/dvm/credentials.json`, where
the SDK finds it (the Python and JS SDKs too). Over SSH, open the link it
prints wherever you like. Where no one is there to approve one, such as CI,
make a key on [dvm.run's API keys page](https://sandbox.dvm.run/keys) and
set `DVM_API_KEY`.

A sandbox records its process from start to end. With no command or
entrypoint it runs the sandbox server and takes commands; the image's own
command does not run. Commands run as root, in the image's environment and
working directory, with any environment you supply; a command's `user`
runs it as another of the image's users. With a command or
entrypoint it is one run of that program, and the new command and file
methods fail locally without sending a request.

```rust
let client = dvm_run::Client::from_env()?;
let sandbox = client.create_sandbox("python:3.12").await?;
sandbox.scope(async |sandbox| {
    let result = sandbox.run(["python", "-c", "print(2)"]).await?;
    assert_eq!(result.exit_code, 0);
    assert_eq!(result.stdout, b"2\n");
    sandbox.write_file("/tmp/a.txt", b"hi").await?;
    assert_eq!(sandbox.read_file("/tmp/a.txt").await?, b"hi");
    let server = sandbox.start("python -m http.server 8000").await?;
    server.signal(15).await?;
    server.wait().await?;
    Ok(())
}).await?;
let mut replay = client.execution(sandbox.id()).await?.replay().await?;
assert!(replay.wait().await?.is_same());
```

`run(command)` takes argv (an array or vector of strings), or a shell
script (`&str` or `String`). Before awaiting, its builder accepts
`.env(name, value)`, `.cwd(path)`, `.user(user)`, and `.stdin(bytes)`. It
sends stdin and closes it, then returns `CommandResult` with `exit_code`,
`stdout: Vec<u8>`, and `stderr: Vec<u8>`. Omitted stdin is empty and closed.

`user` is named as `docker exec --user` names one: a name or uid, optionally
`:group` or `:gid`, looked up in the image's `/etc/passwd` and `/etc/group`.
`HOME` and `USER` are that user's unless you supply them. An unknown user
exits 126 with the reason on stderr.

A sandbox reaches the internet unless made otherwise:
`.network(SandboxNetwork::None)` reaches nothing, not even a resolver, and
`SandboxNetwork::Allowlist { allow }` (names, `*.<domain>`, IPv4 addresses
and networks) the resolver, the internet addresses it gives those names,
and those addresses. It never reaches its runner, the runner's LAN, or
link-local addresses. `set_network(&network)` changes it while the sandbox
runs; open connections stay. A refused connection fails with "Network
unreachable".

`start` takes the same command and builder options and returns `Command`
once started. Its `wait()` gives the result; `signal(number)` signals the
process group. With omitted stdin the handle keeps stdin open:
`write_stdin(&bytes)` sends pieces and `close_stdin()` closes it. Supplied
stdin is sent and closed before `start` returns. Stdin and signal success
means the request was sent; the server sends no acknowledgment. Descendants
may write after the command exits; the result contains output through the
command's exit.

`write_file(path, data).mode(0o600).await?` and `read_file(path).await?`
move whole files as bytes, in pieces of any total size within the sandbox's
limits. Mode is optional and defaults to `0o644`, subject to umask; an
existing file keeps its mode. Parent directories must exist. Writes are
not atomic. A write that fails part-way leaves the file partly written;
the protocol has no way to take it back.

A nonzero command code is a result, not an error. A killing signal is
reported as `128 + signal`; the protocol cannot distinguish it from an
explicit exit with that code. Server refusals return
`Error::SandboxRequest { reason }` with their message. Input flow control
can wait up to 30 seconds, then refuse if the program is not reading;
the SDK passes that refusal on without retrying.

Use only one writer per sandbox. Two separately connected handles writing
at once can corrupt its input. `connect_sandbox(id)` learns the length but
does not lock out another writer. Cloning a `Sandbox` shares its session
and serialized writer.

At scope end, including cancellation, a command sandbox's input is ended
and its exit is waited for, so its recording replays. `end().await?` does
this explicitly and returns the sandbox's exit code. EOF kills commands
still running and makes the server exit 0. A paused sandbox cannot read EOF,
so scope end stops it instead. A supplied program is also stopped if still
active. A stopped recording does not replay. Dropping a sandbox without a
scope leaves it running. Cancelled scopes clean up in a task on the active
tokio runtime; the runtime must remain alive for that cleanup to finish.

- Making a sandbox returns once its process started; one already exited
  is fine. `wait()` gives that process's exit code. A stopped sandbox
  returns `Error::SandboxEnded`; a failed one `Error::SandboxFailed`.
  Pending operations return the same errors if the sandbox ends before
  answering.
- `output()` is raw process output: a command sandbox's stdout is the
  server's JSON answer lines. For a supplied program, dvm merges its
  stdout/stderr in `stdout`; `stderr` is dvm's own diagnostics.
- `pause`, `resume`, `stop`, `state`, and connecting by id act as before.
  A replay verdict is `same`, `other_exit` (both codes), or `other_output`
  (stream and first differing byte). API types are in `dvm_run::api`.
- The address is `DVM_API_URL`, the key is `DVM_API_KEY`, else the key
  `dvm-run login` saved for that address; both can be given in code.
