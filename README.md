# dvm.run SDKs

Clients for [dvm.run](https://dvm.run): sandboxes whose every run
replays. Your agent makes a sandbox from an image, sends it commands and
files, and stops it; everything it ran is its execution, which replays,
the same, once it ends.

| Language | Directory | Package |
| --- | --- | --- |
| Python | [`python/`](python) | [`dvm-run`](https://pypi.org/project/dvm-run/) |
| TypeScript | `js/`, to come | |
| Rust | `rust/`, to come | |

They call the same API and change together, so they live here together.
Each is released on its own tag: `python-v0.1.0`.

## Developing

`nix develop` gives the tools. A directory's tests that need no platform
run anywhere (`cd python && pytest`). Those that need one run against
dvm.run, or a controller and a runner of your own, given `DVM_API_URL` and
`DVM_API_KEY`; dvm-platform's tests run them against its own.

## License

Apache-2.0.
