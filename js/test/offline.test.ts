import assert from "node:assert/strict";
import { test } from "node:test";
import { Connection } from "../src/connection.ts";
import { DvmError, SandboxEndedError, SandboxFailedError } from "../src/errors.ts";
import { Verdict } from "../src/execution.ts";
import { Sandbox } from "../src/sandbox.ts";
import { withEnvironment } from "./environment.ts";

type Answer = (request: Request) => Response | Promise<Response>;
function sandboxAnswering(answer: Answer): Sandbox {
  const fetcher: typeof fetch = async (input, init) => answer(new Request(input, init));
  return new Sandbox(new Connection("http://dvm.test", "test", fetcher), { id: 5, spec: { name: "test" } });
}

test("wait uses versions and returns a nonzero exit", async () => {
  let asked = 0;
  const sandbox = sandboxAnswering((request) => {
    if (asked > 0) assert.equal(new URL(request.url).searchParams.get("newer_than"), "1");
    const state = asked++ === 0 ? { kind: "running" } : { kind: "exited", code: 3 };
    return Response.json({ run: { state, version: 1 } });
  });
  assert.equal(await sandbox.wait(), 3);
});

for (const [state, error] of [
  [{ kind: "stopped" }, SandboxEndedError],
  [{ kind: "failed", reason: "unserved syscall" }, SandboxFailedError],
] as const) {
  test(`wait reports ${state.kind}`, async () => {
    const sandbox = sandboxAnswering(() => Response.json({ run: { state } }));
    await assert.rejects(sandbox.wait(), error);
  });
}

test("output keeps every byte", async () => {
  const bytes = Uint8Array.from({ length: 256 }, (_, byte) => byte);
  const sandbox = sandboxAnswering((request) => {
    assert.equal(new URL(request.url).pathname, "/api/executions/5/output");
    return Response.json({ stdout: Buffer.from(bytes).toString("base64"), stderr: "AA==" });
  });
  assert.deepEqual(await sandbox.output(), { stdout: bytes, stderr: new Uint8Array([0]) });
});

test("create accepts an already exited program", async () => {
  const original = globalThis.fetch;
  globalThis.fetch = async () => Response.json({ id: 5, spec: { name: "test" }, run: { state: { kind: "exited", code: 0 } } });
  try {
    assert.equal((await Sandbox.create("alpine", { command: ["true"], apiKey: "test" })).id, 5);
  } finally {
    globalThis.fetch = original;
  }
});

test("verdict has only program details", () => {
  const verdict = new Verdict({ kind: "other_output", stream: "stderr", at: 7 });
  assert.deepEqual([verdict.stream, verdict.at, verdict.same], ["stderr", 7, false]);
});

test("a key is needed", async () => {
  await withEnvironment({ DVM_API_KEY: undefined, XDG_CONFIG_HOME: "/nonexistent/dvm-test" }, async () => {
    await assert.rejects(Connection.open({ apiUrl: "http://dvm.test" }), DvmError);
  });
});

test("setNetwork puts the mode and an allowlist's entries", async () => {
  const bodies: unknown[] = [];
  const sandbox = sandboxAnswering(async (request) => {
    assert.equal(request.method, "PUT");
    assert.equal(new URL(request.url).pathname, "/api/sandboxes/5/network");
    bodies.push(await request.json());
    return Response.json({ id: 5, spec: { name: "test" } });
  });
  await sandbox.setNetwork("none");
  await sandbox.setNetwork("allowlist", ["pypi.org", "8.8.8.0/24"]);
  assert.deepEqual(bodies, [{ mode: "none" }, { mode: "allowlist", allow: ["pypi.org", "8.8.8.0/24"] }]);
  await assert.rejects(sandbox.setNetwork("public", ["pypi.org"]), DvmError);
});
