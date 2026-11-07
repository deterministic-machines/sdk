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

A sandbox is a VM made from an image: your agent sends it commands and
files until it stops it. Everything it ran is its execution, which
replays, the same, once it ends.

```rust
use std::time::Duration;

let client = dvm_run::Client::from_env()?;
let sbx = client.create_sandbox("python:3.12").seed(7).await?;
sbx.files().write("/app/main.py", "print(1 + 1)\n").await?;
let r = sbx
    .commands()
    .run("python main.py")
    .cwd("/app")
    .timeout(Duration::from_secs(60))
    .await?;
println!("{} {}", r.stdout, r.exit_code);

let server = sbx.commands().spawn(["python", "-m", "http.server"]).await?;
server.kill().await?;
server.wait().await?;
sbx.stop().await?;

let verdict = client.execution(sbx.id()).await?.replay().await?.wait().await?;
assert!(verdict.is_same());
```

`Client::from_env` takes dvm.run's address from `DVM_API_URL`, else
`https://sandbox.dvm.run`, and its key from `DVM_API_KEY`, else the one
`dvm-run login` saved for that address; `Client::new` takes both.

- A `&str` runs in the sandbox's shell (`/bin/bash -lc`, or `/bin/sh -c`
  in an image without bash); an array runs as it is.
- A command's exit code is never an error. A sandbox that ended is
  `Error::SandboxEnded` (`Error::SandboxFailed`, with dvm's reason, if it
  failed) on the next call.
- `.timeout()` kills the command once it passes; its result says so, with
  code 137 and signal 9. A `run` dropped before its command exits kills it
  too; a dropped `wait()` only stops waiting.
- Dropping a sandbox doesn't stop it: call `stop()`, which returns once its
  log is in, so its execution replays.
- The API's types are [`dvm-api`](https://crates.io/crates/dvm-api)'s, as
  `dvm_run::api`.
