/** Frames, as in dvm-api's frame.rs: a JSON object on one line, then the
 * raw bytes it counts in `"bytes"`. */
export const MAX_HEADER = 64 * 1024;
export const MAX_PAYLOAD = 16 * 1024 * 1024;

export class FrameError extends Error {}
export class LongHeader extends FrameError {
  constructor() { super(`a frame's header is longer than ${MAX_HEADER} bytes`); }
}
export class LongPayload extends FrameError {
  readonly length: number;
  constructor(length: number) {
    super(`a frame counts ${length} bytes, more than ${MAX_PAYLOAD}`);
    this.length = length;
  }
}
export class BadHeader extends FrameError {
  constructor(reason: string) { super(`a frame's header: ${reason}`); }
}
export class Cut extends FrameError {
  constructor() { super("the frame ends early"); }
}
export class Trailing extends FrameError {
  readonly length: number;
  constructor(length: number) {
    super(`${length} bytes follow the frame`);
    this.length = length;
  }
}

export interface Frame<H> {
  readonly header: H;
  readonly payload: Uint8Array;
  readonly span: readonly [number, number];
}

type Header = Record<string, unknown>;
const encoder = new TextEncoder();
const decoder = new TextDecoder("utf-8", { fatal: true });

export function encodeFrame(header: object, payload: Uint8Array = new Uint8Array()): Uint8Array {
  if (header === null || Array.isArray(header)) throw new TypeError("a frame's header is a JSON object");
  const line = JSON.stringify(payload.length ? { ...header, bytes: payload.length } : header);
  return join([encoder.encode(line + "\n"), payload]);
}

/** Splits a stream, pushed as it comes, into frames. A bad header throws
 * `BadHeader` for its frame alone; a frame too long to hold ends the stream,
 * and every later call throws the same error. */
export class FrameDecoder {
  #pending: Uint8Array = new Uint8Array();
  #start = 0;
  #offset = 0;
  #ended?: FrameError;

  push(bytes: Uint8Array): void {
    if (this.#start > this.#pending.length / 2) {
      this.#offset += this.#start;
      this.#pending = this.#pending.slice(this.#start);
      this.#start = 0;
    }
    this.#pending = join([this.#pending, bytes]);
  }

  buffered(): number { return this.#pending.length - this.#start; }

  nextFrame<H = Header>(): Frame<H> | undefined {
    if (this.#ended) throw this.#ended;
    const newline = this.#pending.subarray(this.#start, this.#start + MAX_HEADER + 1).indexOf(10);
    if (newline < 0) {
      if (this.buffered() > MAX_HEADER) throw (this.#ended = new LongHeader());
      return undefined;
    }
    const length = newline + 1;
    let header: unknown;
    try {
      header = JSON.parse(decoder.decode(this.#pending.subarray(this.#start, this.#start + newline)));
    } catch (error) {
      this.#take(length);
      throw new BadHeader(String(error));
    }
    const fields = header !== null && typeof header === "object" && !Array.isArray(header) ? header as Header : undefined;
    const count = fields ? fields.bytes ?? 0 : undefined;
    if (!Number.isSafeInteger(count) || (count as number) < 0) {
      this.#take(length);
      throw new BadHeader(`not an object counting its bytes: ${JSON.stringify(header)}`);
    }
    if ((count as number) > MAX_PAYLOAD) throw (this.#ended = new LongPayload(count as number));
    const end = length + (count as number);
    if (this.buffered() < end) return undefined;
    delete fields!.bytes;
    const payload = this.#pending.slice(this.#start + length, this.#start + end);
    return { header: fields as H, payload, span: this.#take(end) };
  }

  #take(length: number): readonly [number, number] {
    const start = this.#offset + this.#start;
    this.#start += length;
    return [start, start + length];
  }
}

/** `bytes`, whole, as one frame. */
export function decodeFrame<H = Header>(bytes: Uint8Array): Frame<H> {
  const decoder = new FrameDecoder();
  decoder.push(bytes);
  const frame = decoder.nextFrame<H>();
  if (!frame) throw new Cut();
  if (decoder.buffered()) throw new Trailing(decoder.buffered());
  return frame;
}

/** The output route's frame: what came of each stream; whether the run may
 * write more; and whether the answer stopped short of what it wrote. */
export interface OutputAfter { stdout: Uint8Array; stderr: Uint8Array; more: boolean; cut: boolean }

export function decodeOutput(bytes: Uint8Array): OutputAfter {
  const { header, payload } = decodeFrame(bytes);
  const { stdout, stderr, more, cut } = header;
  const count = (value: unknown): value is number => Number.isSafeInteger(value) && (value as number) >= 0;
  if (Object.keys(header).length !== 4 || !count(stdout) || !count(stderr) || typeof more !== "boolean"
    || typeof cut !== "boolean" || stdout + stderr !== payload.length) {
    throw new BadHeader(`output ${JSON.stringify(header)} in a frame of ${payload.length} bytes`);
  }
  return { stdout: payload.slice(0, stdout), stderr: payload.slice(stdout), more, cut };
}

export function join(parts: readonly Uint8Array[]): Uint8Array {
  const bytes = new Uint8Array(parts.reduce((size, part) => size + part.length, 0));
  let at = 0;
  for (const part of parts) { bytes.set(part, at); at += part.length; }
  return bytes;
}
