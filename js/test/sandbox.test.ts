import assert from "node:assert/strict";
import { test } from "node:test";
import { AuthenticationError, ConflictError, InvalidRequestError, Sandbox, SandboxEndedError } from "../src/index.ts";
const IMAGE = "alpine:3.20";
const platform = process.env.DVM_API_KEY ? {} : { skip: "needs DVM_API_URL and DVM_API_KEY" };

/** As `await using` does at the scope's end; Node 22 can't parse it. */
async function withSandbox(making: Promise<Sandbox>, use: (sandbox: Sandbox) => Promise<void>): Promise<void> {
  const sandbox = await making;
  try {
    await use(sandbox);
  } finally {
    await sandbox[Symbol.asyncDispose]();
  }
}

test("a program returns its output and exit code and can seek", platform, async () => {
  const command = ["sh", "-c", "printf 'out\\377'; echo err >&2; exit 3"];
  await withSandbox(Sandbox.create(IMAGE, { command, seed: 7 }), async (sandbox) => {
    assert.equal(await sandbox.wait(), 3);
    const output = await sandbox.output();
    assert.deepEqual(output, { stdout: new Uint8Array(Buffer.from("out\xfferr\n", "latin1")), stderr: new Uint8Array() });
    assert.deepEqual([(await sandbox.state()).kind, sandbox.spec.seed, sandbox.spec.name], ["exited", 7, "alpine"]);
    const p1 = await sandbox.seek({ atNs: 0, watch: ["processes"] });
    const p2 = await p1.step();
    assert.ok(p2 !== null);
    assert.notDeepEqual([p2.thread, p2.event], [p1.thread, p1.event]);
    const end = await sandbox.seek();
    assert.ok(end.operations > 0);
    assert.equal((await end.stat("/")).type, "directory");
    assert.ok((await p1.listDir("/")).length > 0);
  });
});

test("entrypoint and environment", platform, async () => {
  const options = { entrypoint: "sh", command: ["-c", 'printf "%s" "$A"'], env: { A: "a  b" } };
  await withSandbox(Sandbox.create(IMAGE, options), async (sandbox) => {
    assert.equal(await sandbox.wait(), 0);
    assert.equal(new TextDecoder().decode((await sandbox.output()).stdout), "a  b");
  });
});

test("pause resume connect and stop", platform, async () => {
  const command = ["sh", "-c", "while :; do :; done"];
  await withSandbox(Sandbox.create(IMAGE, { command, name: "stopped" }), async (sandbox) => {
    const connected = await Sandbox.connect(sandbox.id);
    await connected.pause();
    assert.equal((await sandbox.state()).kind, "paused");
    await connected.resume();
    assert.equal((await sandbox.state()).kind, "running");
    await connected.stop();
    await connected.stop();
    await assert.rejects(sandbox.wait(), SandboxEndedError);
    await assert.rejects(sandbox.seek(), ConflictError);
  });
});

test("refusals are typed", platform, async () => {
  await assert.rejects(Sandbox.create("Not An Image"), InvalidRequestError);
  await assert.rejects(Sandbox.create(IMAGE, { apiKey: "dvm_not-a-key" }), AuthenticationError);
});

test("a command session ends with a whole recording", platform, async () => {
  const path = `/tmp/dvm-js-${crypto.randomUUID()}`;
  let id = 0;
  let capturedSandbox!: Sandbox;
  await withSandbox(Sandbox.create(IMAGE, { env: { A: "inherited" } }), async (sandbox) => {
    capturedSandbox = sandbox;
    id = sandbox.id;
    const result = await sandbox.run(["sh", "-c", "printf '%s ' \"$A\"; pwd; printf err >&2"],
      { env: { A: "command" }, cwd: "/tmp" });
    assert.deepEqual(result, { exitCode: 0, stdout: new TextEncoder().encode("command /tmp\n"), stderr: new TextEncoder().encode("err") });
    assert.equal((await sandbox.run("exit 3")).exitCode, 3);
    assert.equal(new TextDecoder().decode((await sandbox.run('printf "%s" "$A"')).stdout), "inherited");
    const stdin = new Uint8Array([104, 105, 0, 255, 10]);
    assert.deepEqual((await sandbox.run(["cat"], { stdin })).stdout, stdin);
    const data = Uint8Array.from({ length: 102400 }, (_, i) => i % 256);
    await sandbox.writeFile(path, data, { mode: 0o600 });
    assert.deepEqual(await sandbox.readFile(path), data);
    const server = await sandbox.start(["sh", "-c", "while :; do :; done"]);
    assert.equal(new TextDecoder().decode((await sandbox.run("printf concurrent")).stdout), "concurrent");
    await server.signal(15); assert.equal((await server.wait()).exitCode, 143);
    const cat = await sandbox.start(["cat"]);
    await cat.writeStdin(new TextEncoder().encode("from the handle")); await cat.closeStdin();
    assert.equal(new TextDecoder().decode((await cat.wait()).stdout), "from the handle");
    await sandbox.run(["rm", path]);
    assert.match(new TextDecoder().decode((await sandbox.output()).stdout), /"started"/);
    const connected = await Sandbox.connect(id);
    assert.equal(new TextDecoder().decode((await connected.run("printf connected")).stdout), "connected");
  });
  assert.equal((await capturedSandbox.state()).kind, "exited");
  if (process.env.DVM_TEST_SESSION_INSPECT === "1") {
    const start = await capturedSandbox.seek({ atNs: 1, watch: ["file-syscalls"] });
    const next = await start.step();
    assert.ok(next !== null);
    assert.notDeepEqual([next.thread, next.event], [start.thread, start.event]);
    assert.ok((await (await capturedSandbox.seek()).listDir("/")).length > 0);
  }
});

test("new methods refuse a sandbox with a command", platform, async () => {
  await withSandbox(Sandbox.create(IMAGE, { command: ["true"] }), async (sandbox) => {
    for (const call of [() => sandbox.run("true"), () => sandbox.start("true"),
      () => sandbox.writeFile("/tmp/a", new Uint8Array([104, 105])), () => sandbox.readFile("/tmp/a"), () => sandbox.end()]) {
      await assert.rejects(call(), /supplied program/);
    }
  });
});
