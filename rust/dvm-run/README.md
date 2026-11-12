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

A sandbox is one recorded run of one program: the image's own command, or
the command and entrypoint you give as you make it. It ends when the program
exits. Its execution replays, the same, once the program exited.

```rust
let client = dvm_run::Client::from_env()?;
let sandbox = client.create_sandbox("python:3.12")
    .command(["python", "-c", "print(1 + 1)"]).seed(7).await?;
println!("{}", sandbox.wait().await?);
println!("{:?}", sandbox.output().await?.stdout.bytes());
let mut replay = client.execution(sandbox.id()).await?.replay().await?;
assert!(replay.wait().await?.is_same());
println!("{:?}", replay.output().await?.stdout.bytes());
```

- Making a sandbox returns once its program started; one that already
  exited is fine.
- The program's exit code is never an error. `wait()` returns `Error::SandboxEnded` for a sandbox
  that was stopped, and `Error::SandboxFailed`, with dvm's reason, for one
  that failed.
- Output is bytes. In dvm a program's stdout and stderr are one stream, in
  the order it wrote them: `stdout`. `stderr` is what dvm itself said.
- `pause`, `resume` and `stop` act on a sandbox that is going. A stopped
  sandbox's log is cut short, and doesn't replay.
- A replay's verdict is `same`, `other_exit` (both codes) or `other_output`
  (the stream, and the first byte that differs).
- Dropping a sandbox leaves it running. `sandbox.scope(async |sandbox| ..)`
  stops one still going when the scope ends. The API's types are
  `dvm_run::api`.
- It takes dvm.run's address from `DVM_API_URL`, and its key from
  `DVM_API_KEY`, else the one `dvm-run login` saved for that address; both
  may be given in code instead.
