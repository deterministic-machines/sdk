# sandbox-server

`sandbox-server` is an ordinary, single-threaded Linux program, used as the
VM's process 1. It forks commands, moves their bytes, signals their process
groups, and reads and writes files. One epoll loop watches stdin, command
pipes and a signalfd. dvm sees only a program using Linux syscalls;
recording its stdin records the session.

## JSON lines

Each request is one JSON object followed by LF. Its JSON, excluding LF,
is at most 65,536 bytes. An oversized line is discarded through its LF and
gets one error. An unfinished line at EOF gets an error and stops the
server. Answers are also JSON objects followed by LF. Only answer lines
are written to stdout; stderr stays empty. There is no terminal.

Stdout stays blocking. Answers from one pass of the loop are buffered
together and written at the end of that pass. A slow reader slows the
server. A client must read answers while it writes requests; otherwise
both sides can stop waiting for each other.

The types are exported from `dvm-api`: `SandboxRequest`,
`SandboxRequestAction`, `SandboxRun`, `SandboxProgram`, `SandboxStdin`,
`SandboxWrite`, `SandboxRead`, `SandboxAnswer`, `SandboxAnswerKind`,
`SandboxStarted`, and `SandboxExited`. Their TypeScript is included by
`dvm_api::export_typescript`, like the controller types. `ByteChunk`
encodes arbitrary bytes as standard, padded base64; empty bytes are `""`.

Every request has an unsigned 64-bit integer `id`. JavaScript callers
should choose safe integers. A run, read, or first write piece must have
an id greater than every previously started operation's id. The first
id may be zero and gaps are allowed. An accepted start consumes its id
even if the operation fails. A lower or equal id gets an error naming
the last started id. The server keeps only that last id. Pieces of an
open write share its id; stdin and signals refer to a command's id.
Unknown fields, multiple operations on a line, and multiple program
choices are errors. A bad request gets an `error` and
the server continues. The error includes its id when it can be recovered
from a valid JSON object, otherwise it has no id.
An error without an id also aborts every open write, each with an error
under its own id: the lost line may have contained a piece of any write.
A later piece cannot complete an aborted write. Command stdin has the
same risk: after an error without an id, a client cannot tell which
request was lost or whether it contained stdin bytes.

## Requests

`run` starts either a nonempty `argv` (with a nonempty program name) or
a `script`, exactly one. Optional `env` adds or replaces environment
variables in the server's own environment. Optional `cwd` selects the
command's directory; a relative directory starts from the server's
directory. `stdin` defaults to false. Environment names must be nonempty
and contain neither `=` nor NUL; arguments, values and paths cannot
contain NUL.

```json
{"id":1,"run":{"argv":["ls","-l"],"env":{"A":"1"},"cwd":"/repo","stdin":true}}
```

At startup the server selects `/bin/bash -lc` if `/bin/bash` exists,
otherwise `/bin/sh -c` if `/bin/sh` exists, and keeps that choice. With
neither shell a script gets an error; argv commands can still run.

```json
{"id":2,"run":{"script":"ls -l | wc -l"}}
```

The command inherits the server's environment with the additions and
uses that environment's `PATH` to find its program. With no `PATH` it
searches `/bin:/usr/bin`. Empty PATH components refer to the command's
directory. A name containing `/` is used directly. Executable loading
errors produce child stderr and exit 127 for a missing program or 126
for other errors. The server does not interpret an executable text file
that lacks a shebang: an `execve` format error exits 126.

The child has its own process group, an empty signal mask and default
signal dispositions. Its fd 0 is a blocking pipe reader if `stdin` was
true, otherwise `/dev/null`; fds 1 and 2 are separate blocking pipe
writers. Every other descriptor is closed before exec.

`stdin` supplies `data`, `close`, or both. Omitted data is empty and
omitted close is false. Bytes are queued and sent without waiting for
the command to read. A close takes effect after those bytes are sent.
Further input while a close is pending is refused. Input to an already
closed or absent stdin pipe is ignored. Reaping the command closes its
stdin and discards any unsent bytes. For an id up to the last started id
with no live command, input does nothing; a larger id is an error.

```json
{"id":1,"stdin":{"data":"aGk=","close":true}}
```

`signal` sends a number from 1 through 64 to the command's entire process
group. Other numbers always get an error, regardless of the id's state.
An exited command, or an id up to the last started id with no
live command, is ignored; a larger id is an error. Under dvm a handled
signal reaches a command computing without syscalls only at its next
syscall; a signal whose default ends the process lands at once.
Successful stdin and signal requests have no acknowledgment.

```json
{"id":1,"signal":15}
```

`write` supplies a piece of a file. The first piece opens with
`O_CREAT | O_TRUNC`; `mode` defaults to decimal 420 (octal 0644), subject
to the process's umask. An existing file keeps its mode. Later pieces
must use the same path; their mode is ignored. `last` defaults to false.
On last the file closes and `written` is sent. The first file error
closes the write and sends an error; its id stays consumed. No parent
directory is created. Writes are not atomic or rolled back on error.
A piece without an open write must start a new operation with a larger
id; a piece using an old id gets an error naming the last started id.

```json
{"id":3,"write":{"path":"/repo/a","mode":420,"data":"aGk=","last":true}}
```

`read` opens a regular file and streams its bytes. A directory produces
EISDIR, a different nonregular file EINVAL, and a missing file ENOENT.
The server follows symlinks and checks the type before opening, then
checks the opened file again in case the path changed. File errors
include the OS error number; their prose is not a stable identifier.
Paths for reads and writes are relative to the server's directory
unless absolute.

```json
{"id":4,"read":{"path":"/repo/a"}}
```

## Answers and order

`started` is sent first for a command, with its process id and the argv
passed to exec (the shell and its arguments for a script).

```json
{"id":1,"started":{"pid":7,"argv":["ls","-l"]}}
```

`stdout` and `stderr` carry separate chunks of base64 bytes. Chunk
boundaries have no meaning. There is no order promised between the two
streams or between different ids; each stream preserves byte order.

```json
{"id":1,"stdout":"aGk="}
```

```json
{"id":1,"stderr":"ZXJyb3IK"}
```

`exited` is sent when the command's process is reaped, after every byte
that process wrote. Its code is the exit code, or 128 plus the killing
signal. Descendants can keep the pipes open and write under the same id
after `exited`; the server continues reading until the pipes end. There
is no separate pipe-end answer. Multiple commands run concurrently.
The server forgets a command once it is reaped and both pipes end.

```json
{"id":1,"exited":{"code":0}}
```

`written` acknowledges the last piece of a successful write.

```json
{"id":3,"written":true}
```

`data` carries successive file bytes; `end` closes a successful read,
including an empty file. A failed read gets an error instead of end.

```json
{"id":4,"data":"aGk="}
```

```json
{"id":4,"end":true}
```

`error` refuses a request or ends a failed file operation. Already
written bytes remain. It is not a command exit notification.

```json
{"id":3,"error":"No such file or directory (os error 2)"}
```

```json
{"error":"request line exceeds 65536 bytes"}
```

## Lifetime

EOF on the server's stdin, SIGTERM, or SIGINT stops request handling,
sends SIGKILL to every tracked command's process group and to each
command process not yet reaped, and exits 0 without waiting for any
process. Closing stdout's reader also stops the server. During the
session it reaps orphaned descendants as process 1, or as a child
subreaper on the host. Under dvm,
process 1's exit ends the run and every remaining process. On the host,
an orphan that left the tracked groups outlives the server, and whoever
inherits the killed processes reaps them.

The server uses fork, execve, wait4, kill, pipes, epoll, signalfd, and
ordinary file syscalls. At reap it snapshots unread pipe bytes with
FIONREAD so a descendant continually writing cannot postpone `exited`.
It closes child descriptors up to the highest fd in `/proc/self/fd`,
falling back to the descriptor limit when proc is unavailable. It uses
no async runtime or worker threads.

## Build and tests

From the repository root, build a single static x86-64 musl executable:

```sh
nix build .#sandbox-server
file result/bin/sandbox-server
```

That executable needs no libc or dynamic loader in the image. A scratch
image can run argv programs it contains; scripts need one of the shells
above, and disabled command stdin needs `/dev/null`.

Build tests separately from running them:

```sh
nix develop -c cargo test --manifest-path rust/Cargo.toml --all-features --no-run
nix develop -c timeout -k 5 120 cargo test --manifest-path rust/Cargo.toml --locked --all-features
```

Host tests launch the built server through pipes, use bounded polling,
and check only the protocol's ordering. The two dvm tests are ignored by
default. With KVM and the Alpine image available, run each separately:

```sh
export DVM=/path/to/dvm
export SANDBOX_SERVER="$PWD/result/bin/sandbox-server"
nix develop -c cargo test --manifest-path rust/Cargo.toml -p sandbox-server --test dvm --no-run
nix develop -c timeout -k 5 120 cargo test --manifest-path rust/Cargo.toml --locked -p sandbox-server --test dvm recorded_session_replays_identical_answers -- --ignored --exact --nocapture
nix develop -c timeout -k 5 120 cargo test --manifest-path rust/Cargo.toml --locked -p sandbox-server --test dvm session_without_recording -- --ignored --exact --nocapture
```

Each dvm process has a 45-second guest time limit and a 60-second host
wait bound. On failure the test prints captured stdout and stderr. The
recorded test drives a session interactively on one vCPU, then compares
replay stdout byte for byte and compares exit status. It checks a group
member's TERM handler while it waits for a syscall and ends with a
daemon outside the command's group still alive. The unrecorded
test drives the same session to distinguish kernel failures from replay
failures. Each mounts a temporary directory containing only the server.
