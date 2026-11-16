import { FrameDecoder, encodeFrame, join } from "../src/frame.ts";
import assert from "node:assert/strict";
import { test } from "node:test";
import { Connection } from "../src/connection.ts";
import { DvmError, SandboxEndedError, SandboxFailedError } from "../src/errors.ts";
import { Sandbox } from "../src/sandbox.ts";

type RequestLine = { id: number; payload?: Uint8Array; run?: { script?: string; user?: string }; stdin?: { close?: boolean };
  write?: { path: string; mode: number; last: boolean }; read?: { path: string } };
type State = { kind: "running" | "exited" | "failed" | "stopped"; code?: number; reason?: string };
class Mailbox<T> {
  readonly #values: T[] = [];
  readonly #waiting: ((value: T) => void)[] = [];
  put(value: T): void {
    const next = this.#waiting.shift();
    if (next) next(value); else this.#values.push(value);
  }
  next(signal?: AbortSignal): Promise<T> {
    if (this.#values.length) return Promise.resolve(this.#values.shift()!);
    return new Promise((resolve, reject) => {
      const wake = (value: T) => { signal?.removeEventListener("abort", abort); resolve(value); };
      const abort = () => { this.#waiting.splice(this.#waiting.indexOf(wake), 1); reject(signal!.reason); };
      signal?.throwIfAborted();
      signal?.addEventListener("abort", abort, { once: true });
      this.#waiting.push(wake);
    });
  }
}
const bytes = (text: string) => new TextEncoder().encode(text);
const line = (answer: object) => {
  const { payload, ...header } = answer as Record<string, unknown> & { payload?: Uint8Array };
  return encodeFrame(header, payload);
};

class Streams {
  length: number;
  state: State = { kind: "exited", code: 0 };
  readonly requests = new Mailbox<[number, RequestLine]>();
  readonly chunks = new Mailbox<[Uint8Array, boolean]>();
  readonly sandbox: Sandbox;
  probes = 0;
  posts = 0;
  refusal?: string;
  #outputOffset = 0;
  readonly #decoder = new FrameDecoder();
  readonly #base: number;

  constructor(length = 0) {
    this.length = length;
    this.#base = length;
    const fetcher: typeof fetch = (input, init) => this.answer(new Request(input, init));
    this.sandbox = new Sandbox(new Connection("http://dvm.test", "test", fetcher), { id: 5, spec: { name: "test" } });
  }
  async answer(request: Request): Promise<Response> {
    const url = new URL(request.url);
    if (request.method === "POST") {
      assert.equal(request.headers.get("content-type"), "application/octet-stream");
      const data = new Uint8Array(await request.arrayBuffer());
      const offset = Number(url.searchParams.get("offset"));
      const end = url.searchParams.get("end") === "true";
      if (!data.length) { assert.equal(offset, 0); this.probes++; }
      else {
        this.posts++;
        if (this.refusal) return Response.json({ message: this.refusal }, { status: 409 });
        assert.equal(offset, this.length);
        assert.ok(data.length <= 65536);
        this.length += data.length;
        this.#decoder.push(data);
        for (;;) {
          const frame = this.#decoder.nextFrame<RequestLine>();
          if (!frame) break;
          assert.ok(frame.payload.length <= 1024 * 1024);
          if (frame.payload.length) frame.header.payload = frame.payload;
          this.requests.put([this.#base + frame.span[0], frame.header]);
        }
      }
      return Response.json({ length: this.length, ended: end });
    }
    if (url.pathname.endsWith("/output")) {
      assert.equal(Number(url.searchParams.get("offset")), this.#outputOffset);
      const [data, more] = await this.chunks.next(request.signal);
      this.#outputOffset += data.length;
      return new Response(encodeFrame({ stdout: data.length, stderr: 0, more, cut: false }, data), { headers: { "content-type": "application/octet-stream" } });
    }
    return Response.json({ run: { state: this.state } });
  }
  emit(...answers: object[]): void {
    this.chunks.put([join(answers.map(line)), true]);
  }
  async start(name: string) {
    const pending = this.sandbox.start(name);
    const [offset, request] = await this.requests.next();
    assert.equal(request.id, offset);
    this.emit({ id: offset, started: { pid: 1, argv: [name] } });
    return [await pending, offset] as const;
  }
  async close(): Promise<void> { await this.sandbox[Symbol.asyncDispose](); }
}

test("split lines and interleaved commands use input offsets", async () => {
  const streams = new Streams(123);
  const first = streams.sandbox.start(["first"]);
  const [aid, aRequest] = await streams.requests.next();
  assert.equal(aid, 123); assert.equal(aRequest.id, aid); assert.ok(!("user" in aRequest.run!));
  const second = streams.sandbox.start("second", { user: "agent" });
  const [bid, bRequest] = await streams.requests.next();
  assert.ok(bid > aid); assert.equal(bRequest.id, bid); assert.equal(bRequest.run?.script, "second");
  assert.equal(bRequest.run?.user, "agent");
  const answers = Buffer.concat([line({ id: aid, started: { pid: 1, argv: ["first"] } }),
    line({ id: bid, started: { pid: 2, argv: ["sh", "-c", "second"] } }), line({ id: aid, stdout: true, payload: bytes("a") })]);
  streams.chunks.put([answers.subarray(0, -4), true]);
  streams.chunks.put([answers.subarray(-4), true]);
  const [a, b] = await Promise.all([first, second]);
  const data = Uint8Array.from({ length: 2304000 }, (_, i) => i % 256);
  const writing = a.writeStdin(data);
  const received: Buffer[] = [];
  let count = 0;
  while (count < data.length) {
    const [, request] = await streams.requests.next();
    assert.equal(request.id, aid);
    const piece = Buffer.from(request.payload!);
    received.push(piece); count += piece.length;
  }
  await writing;
  assert.deepEqual(new Uint8Array(Buffer.concat(received)), data);
  await a.closeStdin();
  assert.deepEqual((await streams.requests.next())[1], { id: aid, stdin: { close: true } });
  streams.emit({ id: bid, stdout: true, payload: bytes("b") }, { id: aid, stderr: true, payload: bytes("err") },
    { id: bid, exited: { code: 3 } }, { id: aid, exited: { code: 0 } });
  assert.deepEqual(await a.wait(), { exitCode: 0, stdout: bytes("a"), stderr: bytes("err") });
  assert.deepEqual(await b.wait(), { exitCode: 3, stdout: bytes("b"), stderr: new Uint8Array() });
  assert.equal(streams.probes, 1);
  await streams.close();
});

for (const failure of ["id", "no-id", "exit", "failed"]) {
  test(`errors dispatch to one or all operations: ${failure}`, async () => {
    const streams = new Streams();
    streams.state = failure === "failed" ? { kind: "failed", reason: "recording limit" } : { kind: "exited", code: 7 };
    const [a, aid] = await streams.start("first");
    const [b, bid] = await streams.start("second");
    if (failure === "id") {
      streams.emit({ id: aid, error: "refused command" }, { id: bid, exited: { code: 0 } });
      await assert.rejects(a.wait(), /refused command/);
      assert.equal((await b.wait()).exitCode, 0);
    } else {
      if (failure === "no-id") streams.emit({ error: "bad line" });
      else streams.chunks.put([new Uint8Array(), false]);
      const error = failure === "no-id" ? /bad line/ : failure === "failed" ? SandboxFailedError : SandboxEndedError;
      await assert.rejects(a.wait(), error); await assert.rejects(b.wait(), error);
      if (failure === "no-id") {
        const [following, id] = await streams.start("following");
        streams.emit({ id, exited: { code: 0 } });
        assert.equal((await following.wait()).exitCode, 0);
      }
    }
    await streams.close();
  });
}

test("files are cut into bounded pieces and read back", async () => {
  const streams = new Streams();
  const data = Uint8Array.from({ length: 2304000 }, (_, i) => i % 256);
  const writing = streams.sandbox.writeFile("/tmp/a", data, { mode: 0o600 });
  const received: Buffer[] = [];
  let id: number | undefined;
  for (;;) {
    const [, request] = await streams.requests.next();
    id ??= request.id;
    assert.equal(request.id, id);
    const piece = request.write!;
    assert.deepEqual([piece.path, piece.mode], ["/tmp/a", 0o600]);
    received.push(Buffer.from(request.payload!));
    if (piece.last) break;
  }
  assert.deepEqual(new Uint8Array(Buffer.concat(received)), data);
  streams.emit({ id, written: true }); await writing;
  const reading = streams.sandbox.readFile("/tmp/a");
  const [rid, request] = await streams.requests.next();
  assert.ok(rid > id!); assert.deepEqual(request, { id: rid, read: { path: "/tmp/a" } });
  for (let at = 0; at < data.length; at += 8000) streams.emit({ id: rid, data: true, payload: data.subarray(at, at + 8000) });
  streams.emit({ id: rid, end: true });
  assert.deepEqual(await reading, data);
  await streams.close();
});

test("program sandboxes refuse locally and backpressure is not retried", async () => {
  let calls = 0;
  const fetcher: typeof fetch = async () => { calls++; throw new Error("unexpected HTTP"); };
  const program = new Sandbox(new Connection("http://dvm.test", "test", fetcher), { id: 5, spec: { name: "test", command: ["cat"] } });
  for (const call of [() => program.run("true"), () => program.start("true"),
    () => program.writeFile("a", new Uint8Array()), () => program.readFile("a"), () => program.end()]) {
    await assert.rejects(call(), /supplied program/);
  }
  assert.equal(calls, 0);
  const streams = new Streams();
  streams.state = { kind: "running" };
  streams.refusal = "the program is not reading its input; input length 0";
  await assert.rejects(streams.sandbox.run("true"), /not reading its input/);
  assert.equal(streams.posts, 1);
  streams.state = { kind: "stopped" };
  await streams.close();
});

for (const kind of ["session", "paused", "program", "explicit"]) {
  test(`scope cleanup: ${kind}`, async () => {
    const calls: string[] = [];
    let state: { kind: string; code?: number } = { kind: kind === "paused" ? "paused" : "running" };
    const fetcher: typeof fetch = async (input, init) => {
      const request = new Request(input, init);
      const path = new URL(request.url).pathname;
      if (request.method === "GET") return path.endsWith("/output")
        ? new Response(encodeFrame({ stdout: 0, stderr: 0, more: false, cut: false })) : Response.json({ run: { state } });
      calls.push(path);
      if (path.endsWith("/input")) {
        assert.equal((await request.arrayBuffer()).byteLength, 0);
        assert.equal(new URL(request.url).search, "?offset=0&end=true");
        state = { kind: "exited", code: 0 };
        return Response.json({ length: 0, ended: true });
      }
      state = { kind: "stopped" }; return Response.json(null);
    };
    const sandbox = new Sandbox(new Connection("http://dvm.test", "test", fetcher),
      { id: 5, spec: { name: "test", command: kind === "program" ? ["cat"] : [] } }, 0);
    if (kind === "explicit") assert.equal(await sandbox.end(), 0);
    await sandbox[Symbol.asyncDispose]();
    assert.deepEqual(calls, [`/api/sandboxes/5/${["paused", "program"].includes(kind) ? "stop" : "input"}`]);
  });
}
