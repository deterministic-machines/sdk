/** The SDK against dvm.run, or a controller and a runner, at DVM_API_URL
 * with DVM_API_KEY: dvm-platform's tests start them, and run these, as they
 * run the Python SDK's. */

import assert from "node:assert/strict";
import { test } from "node:test";

import {
  AuthenticationError,
  ConflictError,
  Execution,
  InvalidRequestError,
  NotFoundError,
  Sandbox,
  SandboxEndedError,
} from "../src/index.ts";

// dvm-platform's tests run these against a stand-in sandbox, which runs
// commands on the host from a directory of its own, so paths in these are
// relative to it, as they are to alpine's working directory, `/`.
const IMAGE = "alpine:3.20";

const platform = process.env.DVM_API_KEY
  ? {}
  : { skip: "needs dvm.run, or a controller and a runner: DVM_API_URL and DVM_API_KEY" };

/** A test with a sandbox of its own, stopped at its end. */
function sandboxTest(name: string, run: (sandbox: Sandbox) => Promise<void>): void {
  test(name, platform, async () => {
    const sandbox = await Sandbox.create(IMAGE);
    try {
      await run(sandbox);
    } finally {
      await sandbox.stop();
    }
  });
}

sandboxTest("a command returns its output and exit code", async (sandbox) => {
  const script = await sandbox.commands.run("echo out; echo err >&2; exit 3");
  assert.deepEqual(
    [script.stdout, script.stderr, script.exitCode, script.signal],
    ["out\n", "err\n", 3, null],
  );
  assert.equal((await sandbox.commands.run(["echo", "a  b"])).stdout, "a  b\n");
  assert.equal((await sandbox.commands.run("echo $A", { env: { A: "1" } })).stdout, "1\n");
});

sandboxTest("output comes to callbacks as it is written", async (sandbox) => {
  const pieces: string[] = [];
  const counted = await sandbox.commands.run("for i in 1 2 3; do echo $i; sleep 0.02; done", {
    onStdout: (text) => pieces.push(text),
  });
  assert.equal(pieces.join(""), "1\n2\n3\n");
  assert.equal(counted.stdout, "1\n2\n3\n");
});

sandboxTest("a background command takes stdin until its close", async (sandbox) => {
  const cat = await sandbox.commands.run(["cat"], { background: true, stdin: true });
  await cat.sendStdin(new TextEncoder().encode("one\n"));
  await cat.sendStdin("two\n");
  await cat.closeStdin();
  assert.equal((await cat.wait()).stdout, "one\ntwo\n");
  assert.equal((await cat.info()).state, "exited");
  assert.equal((await sandbox.commands.run(["cat"], { stdin: "piped" })).stdout, "piped");
});

sandboxTest("a killed command says its signal", async (sandbox) => {
  const sleeper = await sandbox.commands.run(["sleep", "30"], { background: true });
  await sleeper.kill();
  const killed = await sleeper.wait();
  assert.deepEqual([killed.exitCode, killed.signal], [137, 9]);
  const timedOut = await sandbox.commands.run(["sleep", "30"], { timeoutMs: 200 });
  assert.deepEqual([timedOut.exitCode, timedOut.signal], [137, 9]);
});

sandboxTest("a file goes in and out", async (sandbox) => {
  const data = Uint8Array.from({ length: 256 }, (_, byte) => byte);
  await sandbox.files.write("/data.bin", data);
  assert.deepEqual(await sandbox.files.read("/data.bin"), data);
  await sandbox.files.write("/run.sh", "#!/bin/sh\necho ran\n", { mode: 0o755 });
  assert.equal((await sandbox.commands.run("./run.sh")).stdout, "ran\n");
  assert.equal(await sandbox.files.readText("/run.sh"), "#!/bin/sh\necho ran\n");
  await assert.rejects(sandbox.files.read("/missing"), NotFoundError);
});

sandboxTest("a paused sandbox takes no commands until resumed", async (sandbox) => {
  await sandbox.pause();
  assert.equal((await sandbox.state()).kind, "paused");
  await assert.rejects(sandbox.commands.run("true"), ConflictError);
  await sandbox.resume();
  assert.equal((await sandbox.commands.run("echo back")).stdout, "back\n");
});

sandboxTest("another process connects by id", async (sandbox) => {
  const connected = await Sandbox.connect(sandbox.id);
  assert.equal((await connected.commands.run("echo here")).stdout, "here\n");
});

test("a stopped sandbox says it ended", platform, async () => {
  const sandbox = await Sandbox.create(IMAGE, { name: "stopped" });
  assert.equal(sandbox.name, "stopped");
  await sandbox.stop();
  await sandbox.stop();
  await assert.rejects(sandbox.commands.run("true"), (error) => {
    assert.ok(error instanceof SandboxEndedError);
    assert.equal(error.state.kind, "stopped");
    return true;
  });
});

test("an execution replays the same", platform, async () => {
  const sandbox = await Sandbox.create(IMAGE, { seed: 7 });
  try {
    await sandbox.files.write("/note.txt", "hello\n");
    assert.equal((await sandbox.commands.run("cat note.txt")).stdout, "hello\n");
    assert.equal((await sandbox.commands.run("exit 2")).exitCode, 2);
  } finally {
    // As `await using` does at the scope's end.
    await sandbox[Symbol.asyncDispose]();
  }
  const execution = await Execution.get(sandbox.id);
  assert.deepEqual(
    [execution.state.kind, execution.spec.seed, execution.spec.name],
    ["stopped", 7, "alpine"],
  );
  assert.notEqual(execution.logBytes, null);
  assert.deepEqual(
    execution.commands.map((command) => [command.number, command.state, command.exitCode]),
    [
      [1, "exited", 0],
      [2, "exited", 2],
    ],
  );
  const replay = await execution.replay();
  assert.ok((await replay.wait()).same);
});

test("refusals are typed", platform, async () => {
  await assert.rejects(Sandbox.create("Not An Image"), InvalidRequestError);
  await assert.rejects(Sandbox.create(IMAGE, { apiKey: "dvm_not-a-key" }), AuthenticationError);
});
