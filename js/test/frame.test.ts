import assert from "node:assert/strict";
import { test } from "node:test";
import {
  MAX_HEADER, MAX_PAYLOAD, BadHeader, Cut, FrameDecoder, LongHeader, LongPayload,
  Trailing, decodeFrame, decodeOutput, encodeFrame, join,
} from "../src/frame.ts";

const bytes = (text: string) => new TextEncoder().encode(text);
function take(decoder: FrameDecoder) {
  const frames = [];
  for (let frame = decoder.nextFrame(); frame; frame = decoder.nextFrame()) frames.push(frame);
  return frames;
}

test("frames carry raw bytes and split wherever the stream is cut", () => {
  const payload = join([bytes('\n{"id":9}\n'), new Uint8Array([255])]);
  const stream = join([encodeFrame({ id: 1 }, payload), encodeFrame({ id: 2 })]);
  assert.deepEqual(stream.slice(0, 23), bytes('{"id":1,"bytes":11}\n\n{"'));
  const expected = [
    { header: { id: 1 }, payload, span: [0, 31] },
    { header: { id: 2 }, payload: new Uint8Array(), span: [31, 40] },
  ];
  for (let cut = 0; cut <= stream.length; cut++) {
    const decoder = new FrameDecoder();
    decoder.push(stream.subarray(0, cut));
    const frames = take(decoder);
    decoder.push(stream.subarray(cut));
    assert.deepEqual([...frames, ...take(decoder)], expected);
    assert.equal(decoder.buffered(), 0);
  }
});

test("a bad header costs its frame and a long one ends the stream", () => {
  const decoder = new FrameDecoder();
  decoder.push(bytes('not json\n[1]\n{"bytes":-1}\n{"bytes":1.5}\n{"bytes":true}\n{"id":4}\n'));
  for (let i = 0; i < 5; i++) assert.throws(() => decoder.nextFrame(), BadHeader);
  assert.deepEqual(decoder.nextFrame()?.header, { id: 4 });
  assert.throws(() => decodeFrame(new Uint8Array([123, 34, 120, 34, 58, 34, 255, 34, 125, 10])), BadHeader);

  decoder.push(bytes(`{"id":5,"bytes":${MAX_PAYLOAD + 1}}\n`));
  assert.throws(() => decoder.nextFrame(), LongPayload);
  decoder.push(bytes('{"id":6}\n'));
  assert.throws(() => decoder.nextFrame(), LongPayload);

  const long = new FrameDecoder();
  long.push(bytes(" ".repeat(MAX_HEADER)));
  assert.equal(long.nextFrame(), undefined);
  long.push(bytes(" "));
  assert.throws(() => long.nextFrame(), LongHeader);
});

test("a whole frame is one frame and nothing more", () => {
  const frame = encodeFrame({ id: 1 }, bytes("hi"));
  assert.deepEqual(decodeFrame(frame).payload, bytes("hi"));
  assert.throws(() => decodeFrame(frame.slice(0, -1)), Cut);
  assert.throws(() => decodeFrame(join([frame, frame])), (error) => error instanceof Trailing && error.length === frame.length);
  assert.deepEqual(encodeFrame({}, bytes("x")), bytes('{"bytes":1}\nx'));
});

test("output counts match the payload", () => {
  const payload = new Uint8Array([0, 255, 101, 114, 114]);
  assert.deepEqual(decodeOutput(encodeFrame({ stdout: 2, stderr: 3, more: false, cut: true }, payload)), {
    stdout: payload.slice(0, 2), stderr: payload.slice(2), more: false, cut: true,
  });
  for (const header of [
    { stdout: 9, stderr: 0, more: true, cut: false }, { stdout: 2, stderr: 0, more: 1, cut: false },
    { stdout: 2, stderr: 0, more: true },
  ]) assert.throws(() => decodeOutput(encodeFrame(header, bytes("hi"))), BadHeader);
});
