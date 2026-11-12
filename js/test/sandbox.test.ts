import assert from "node:assert/strict";
import { test } from "node:test";
import { AuthenticationError, ConflictError, Execution, InvalidRequestError, Sandbox, SandboxEndedError } from "../src/index.ts";
const IMAGE = "alpine:3.20";
const platform = process.env.DVM_API_KEY ? {} : { skip: "needs DVM_API_URL and DVM_API_KEY" };

test("a program returns its output and exit code and replays", platform, async () => {
  await using sandbox = await Sandbox.create(IMAGE, { command: ["sh", "-c", "printf 'out\\377'; echo err >&2; exit 3"], seed: 7 });
  assert.equal(await sandbox.wait(), 3);
  const output = await sandbox.output();
  assert.deepEqual(output, { stdout: new Uint8Array(Buffer.from("out\xfferr\n", "latin1")), stderr: new Uint8Array() });
  const execution = await Execution.get(sandbox.id);
  assert.deepEqual([execution.state.kind, execution.spec.seed, execution.spec.name], ["exited", 7, "alpine"]);
  assert.notEqual(execution.logBytes, null);
  const replay = await execution.replay();
  assert.ok((await replay.wait()).same);
  assert.deepEqual(await replay.output(), output);
});

test("entrypoint and environment", platform, async () => {
  await using sandbox = await Sandbox.create(IMAGE, { entrypoint: "sh", command: ["-c", 'printf "%s" "$A"'], env: { A: "a  b" } });
  assert.equal(await sandbox.wait(), 0);
  assert.equal(new TextDecoder().decode((await sandbox.output()).stdout), "a  b");
});

test("an image runs its own command", platform, async () => {
  await using sandbox = await Sandbox.create(IMAGE);
  assert.equal(await sandbox.wait(), 0);
});

test("pause resume connect and stop", platform, async () => {
  await using sandbox = await Sandbox.create(IMAGE, { command: ["sh", "-c", "while :; do :; done"], name: "stopped" });
  const connected = await Sandbox.connect(sandbox.id);
  await connected.pause();
  assert.equal((await sandbox.state()).kind, "paused");
  await connected.resume();
  assert.equal((await sandbox.state()).kind, "running");
  await connected.stop();
  await connected.stop();
  await assert.rejects(sandbox.wait(), SandboxEndedError);
  await assert.rejects((await Execution.get(sandbox.id)).replay(), ConflictError);
});

test("refusals are typed", platform, async () => {
  await assert.rejects(Sandbox.create("Not An Image"), InvalidRequestError);
  await assert.rejects(Sandbox.create(IMAGE, { apiKey: "dvm_not-a-key" }), AuthenticationError);
});
