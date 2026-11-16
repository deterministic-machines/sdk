# dvm.run SDKs

Clients for [dvm.run](https://dvm.run): sandboxes whose every run
replays. With no command or entrypoint, a sandbox takes commands and moves
files through its SDK session. With either supplied, it is one run of that
program. The whole session is one recorded execution, which replays once
its input ends and the sandbox exits.

| Language | Directory | Package |
| --- | --- | --- |
| Python 3.12+ | [`python/`](python) | [`dvm-run`](https://pypi.org/project/dvm-run/) |
| JavaScript, TypeScript | [`js/`](js) | [`dvm-run`](https://www.npmjs.com/package/dvm-run) |
| Rust | [`rust/`](rust) | [`dvm-run`](https://crates.io/crates/dvm-run), over [`dvm-api`](https://crates.io/crates/dvm-api) |

They call the same API and change together, so they live here together.
Each is released on its own tag: `python-v0.3.0`, `js-v0.3.0`,
`rust-v0.3.0`. The Rust crates hold the API's types (`dvm-api`), which
dvm.run's controller is built over.

## Bytes on the wire

Sandbox requests and answers use a JSON header ending in LF, followed by
raw bytes when the header contains `"bytes": N`. Empty payloads omit that
field. Headers are at most 64 KiB excluding LF; payloads are at most
16 MiB. [The sandbox protocol](rust/sandbox-server/README.md) describes
the operations. Each language has one frame codec matching
[`dvm-api`'s codec](rust/dvm-api/src/frame.rs).

`POST /api/sandboxes/{id}/input?offset=N&end=true|false` appends a raw
`application/octet-stream` body of at most 64 KiB. Offsets count the
whole input stream, including frame headers. The SDKs split stdin and
file writes into payloads of at most 1 MiB, then split each frame across
HTTP input bodies as needed.

`GET /api/executions/{id}/output?offset=N&stderr_offset=M` returns
`application/octet-stream`: one frame with a header containing stdout
and stderr lengths and `more`, followed by stdout bytes then stderr
bytes. For example, `{"stdout":2,"stderr":0,"more":false,"bytes":2}`
followed by LF and `hi`. Each response holds at most 4 MiB per stream.
The SDKs advance both offsets and keep polling until `more` is false.
A session's stdout contains sandbox answer frames; a supplied program's
stdout contains its output directly. Bytes remain raw across the sandbox
protocol, runner–controller connection, and SDK HTTP API. `ByteChunk`
remains for the panel's JSON APIs.

## Developing

`nix develop` gives the tools. A directory's tests that need no platform
run anywhere (`nix develop -c bash -c "cd python && pytest"`,
`nix develop -c bash -c "cd js && npm ci && npm test"`,
`nix develop -c bash -c "cd rust && cargo test"`). Those that need a
platform run against dvm.run, or a controller and runner of your own, given `DVM_API_URL` and
`DVM_API_KEY` (Rust's with `cargo test -- --ignored`); dvm-platform's
tests run them against its own.

## License

Apache-2.0.
