/** The SDK against answers of the test's own: what needs no platform. */

import assert from "node:assert/strict";
import { test } from "node:test";

import { CommandHandle } from "../src/commands.ts";
import { Connection } from "../src/connection.ts";
import { DvmError, SandboxFailedError } from "../src/errors.ts";
import { momentOf } from "../src/execution.ts";
import { Sandbox } from "../src/sandbox.ts";
import { SandboxCalls } from "../src/sandbox-calls.ts";
import { withEnvironment } from "./environment.ts";

type Answer = (request: Request) => Response | Promise<Response>;

function fetchAnswering(answer: Answer): typeof fetch {
  return async (input, init) => answer(new Request(input, init));
}

function connectionAnswering(answer: Answer): Connection {
  return new Connection("http://dvm.test", "dvm_test", fetchAnswering(answer));
}

function commandAnswering(answer: Answer): CommandHandle {
  return new CommandHandle(new SandboxCalls(connectionAnswering(answer), 5), 1);
}

function events(...sent: unknown[]): Response {
  const body = sent.map((event) => `: kept alive\n\ndata: ${JSON.stringify(event)}\n\n`).join("");
  return new Response(body, { headers: { "content-type": "text/event-stream" } });
}

function output(stream: string, offset: number, chunk: Uint8Array) {
  return { kind: "output", stream, offset, chunk: Buffer.from(chunk).toString("base64") };
}

function command(state: unknown) {
  return {
    kind: "command",
    command: { number: 1, command: "printf", argv: null, state, started_at: null, ended_at: null },
  };
}

test("a dropped output stream is followed again from where it got to", async () => {
  const asked: [string | null, string | null][] = [];
  const eAcute = new TextEncoder().encode("é");
  const handle = commandAnswering((request) => {
    const url = new URL(request.url);
    assert.equal(url.pathname, "/api/sandboxes/5/commands/1/output");
    asked.push([url.searchParams.get("stdout_offset"), url.searchParams.get("stderr_offset")]);
    if (asked.length === 1) {
      return events(output("stdout", 0, new TextEncoder().encode("hel")), output("stderr", 0, eAcute.subarray(0, 1)));
    }
    return events(
      output("stdout", 3, new TextEncoder().encode("lo")),
      output("stderr", 1, eAcute.subarray(1)),
      command({ kind: "exited", code: 0 }),
    );
  });
  const stderr: string[] = [];
  const result = await handle.wait({ onStderr: (text) => stderr.push(text) });
  assert.deepEqual([result.stdout, result.stderr, stderr], ["hello", "é", ["é"]]);
  assert.deepEqual(asked, [["0", "0"], ["3", "1"]]);
});

test("a stream that never ends its command is an error", async () => {
  const handle = commandAnswering(() => events(command({ kind: "running" })));
  await assert.rejects(handle.wait(), /ended before its command/);
});

test("a call refused as its sandbox failed says why", async () => {
  const connection = connectionAnswering((request) => {
    if (request.method === "POST") {
      return Response.json({ message: "sandbox 5 is failed" }, { status: 409 });
    }
    const failed = { kind: "failed", reason: "syscall 999 isn't served" };
    return Response.json({ id: 5, run: { state: failed } });
  });
  const sandbox = new Sandbox(connection, { id: 5, spec: { name: "test" } });
  await assert.rejects(sandbox.commands.run("true"), (error) => {
    assert.ok(error instanceof SandboxFailedError);
    assert.match(error.message, /syscall 999/);
    return true;
  });
});

test("an aborted command is killed, and a timed-out one says its signal", async () => {
  const signals: unknown[] = [];
  let whenRunning = () => {};
  const connection = connectionAnswering(async (request) => {
    const path = new URL(request.url).pathname;
    if (path === "/api/sandboxes/5/commands") return Response.json({ number: 1 }, { status: 201 });
    if (path === "/api/sandboxes/5/commands/1/signal") {
      signals.push(await request.json());
      return Response.json(null);
    }
    if (signals.length > 0) return events(command({ kind: "exited", code: 137, signal: 9 }));
    // Running until it's killed: a stream that stays open, until its request's end.
    const open = new ReadableStream({
      start(controller) {
        request.signal.addEventListener("abort", () => controller.error(request.signal.reason));
      },
    });
    setImmediate(whenRunning);
    return new Response(open, { headers: { "content-type": "text/event-stream" } });
  });
  const sandbox = new Sandbox(connection, { id: 5, spec: { name: "test" } });

  const giveUp = new AbortController();
  whenRunning = () => giveUp.abort();
  await assert.rejects(sandbox.commands.run("sleep 30", { signal: giveUp.signal }), {
    name: "AbortError",
  });
  assert.deepEqual(signals, [{ signal: 9 }]);

  signals.length = 0;
  whenRunning = () => {};
  const timedOut = await sandbox.commands.run("sleep 30", { timeoutMs: 1 });
  assert.deepEqual([timedOut.exitCode, timedOut.signal, signals], [137, 9, [{ signal: 9 }]]);
});

test("a background command takes its timeout and callbacks in wait", async () => {
  const sandbox = new Sandbox(connectionAnswering(() => Response.json(null)), { id: 5, spec: { name: "test" } });
  await assert.rejects(
    sandbox.commands.run("true", { background: true, timeoutMs: 1 } as never),
    TypeError,
  );
});

test("a moment is read as dvm writes it", () => {
  assert.deepEqual(momentOf("3:17"), { kind: "event", thread: 3, operation: 17 });
  assert.deepEqual(momentOf("@12.5"), { kind: "time", at_us: 12500 });
  assert.deepEqual(momentOf("@7"), { kind: "time", at_us: 7000 });
});

test("a key is needed", async () => {
  const nowhere = { DVM_API_KEY: undefined, XDG_CONFIG_HOME: "/nonexistent/dvm-test" };
  await withEnvironment(nowhere, async () => {
    await assert.rejects(Connection.open({ apiUrl: "http://dvm.test" }), (error) => {
      assert.ok(error instanceof DvmError);
      assert.match(error.message, /DVM_API_KEY/);
      return true;
    });
  });
});
