# dvm-api

[dvm.run](https://dvm.run)'s REST API: what it takes and returns, checked.
dvm.run's controller serves it, and the Rust SDK,
[`dvm-run`](https://crates.io/crates/dvm-run), calls it; both check a
sandbox's spec with the same code. Use `dvm-run` to call dvm.run.

`frame` defines the shared wire codec: a JSON object ending in LF, then
its counted raw payload (`bytes`, omitted when empty). Headers are
limited to 64 KiB and payloads to 16 MiB. Bad headers consume one frame;
oversized headers and payloads end decoding. `SandboxInput` is an HTTP
query with a raw body, and `OutputAfter` converts the HTTP output frame
to and from stdout and stderr bytes. `ByteChunk` serves the panel's JSON
APIs. See the [wire format](../../README.md#bytes-on-the-wire).
