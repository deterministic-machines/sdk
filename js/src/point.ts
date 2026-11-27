import type {
  ApiDecodedArgument,
  ApiDvmStop,
  ApiFileStatus,
  ApiPathDirectoryEntry,
  ApiSeekTarget,
  ApiStopEvent,
  ApiWatchPoint,
} from "./api-json.ts";
import { type CallOptions, type Connection, type Query } from "./connection.ts";
import { DvmError } from "./errors.ts";
import { decodeOutput } from "./frame.ts";

const NAMED_WATCH_POINTS = new Map<string, ApiWatchPoint>([
  ["syscall", { kind: "syscall" }],
  ["page-fault", { kind: "page_fault" }],
  ["page_fault", { kind: "page_fault" }],
  ["processes", { kind: "processes" }],
  ["file-syscalls", { kind: "file_syscalls" }],
  ["file_syscalls", { kind: "file_syscalls" }],
  ["socket-syscalls", { kind: "socket_syscalls" }],
  ["socket_syscalls", { kind: "socket_syscalls" }],
]);

/** What the process wrote: raw bytes on stdout and stderr. */
export interface Output {
  stdout: Uint8Array;
  stderr: Uint8Array;
}

export async function readOutput(connection: Connection, path: string, options: CallOptions = {}): Promise<Output> {
  const stdout: Uint8Array[] = [];
  const stderr: Uint8Array[] = [];
  let offset = 0, stderrOffset = 0;
  let query: { offset: number; stderr_offset: number } | undefined;
  for (;;) {
    const page = decodeOutput(await connection.bytes(path, { ...options, query }));
    stdout.push(page.stdout);
    stderr.push(page.stderr);
    offset += page.stdout.length;
    stderrOffset += page.stderr.length;
    if (!page.cut) return { stdout: join(stdout, offset), stderr: join(stderr, stderrOffset) };
    query = { offset, stderr_offset: stderrOffset };
  }
}

export type WatchInput = ApiWatchPoint | string;

export interface SeekOptions extends CallOptions {
  atNs?: number;
  thread?: number;
  event?: number;
  watch?: readonly WatchInput[];
}

export interface StepOptions extends CallOptions {
  watch?: readonly WatchInput[];
}

export interface StatOptions extends CallOptions {
  followSymlinks?: boolean;
}

export interface DecodedArgument {
  arg: number;
  kind: "path" | "strings" | "address" | "bytes";
  value: string | string[];
  truncated: boolean;
}

export interface StopEvent {
  kind: "syscall" | "page_fault";
  syscall?: number;
  name?: string;
  args?: string[];
  decoded?: DecodedArgument[];
  address?: string;
  write?: boolean;
  present?: boolean;
}

export interface DirEntry {
  name: string;
  rawName: Uint8Array;
  type: string;
}

export interface FileStat {
  type: string;
  mode: number;
  uid: number;
  gid: number;
  size: number;
  links: number;
  mtime: [number, number];
  ctime: [number, number];
  target: Uint8Array | null;
  mount: string | null;
}

export class SandboxPoint {
  readonly sandboxId: number;
  readonly thread: number;
  readonly process: number;
  readonly operations: number;
  readonly event: number;
  readonly ns: number | null;
  readonly at: StopEvent;
  readonly stats: Record<string, number>;
  readonly #connection: Connection;

  constructor(
    connection: Connection,
    sandboxId: number,
    stop: ApiDvmStop,
  ) {
    this.#connection = connection;
    this.sandboxId = sandboxId;
    this.thread = stop.thread;
    this.process = stop.process;
    this.operations = stop.operations;
    this.event = stop.event;
    this.ns = stop.recording_ns;
    this.at = parseStopEvent(stop.at);
    this.stats = { ...stop.stats };
  }

  async step(options: StepOptions = {}): Promise<SandboxPoint | null> {
    const body = { thread: this.thread, event: this.event, ...encodeWatches(options.watch) };
    const stop = await this.#connection.json<ApiDvmStop | null>(
      "POST",
      `/sandboxes/${this.sandboxId}/points/step`,
      { signal: options.signal, body },
    );
    return stop === null ? null : new SandboxPoint(this.#connection, this.sandboxId, stop);
  }

  readFile(path: string, options: CallOptions = {}): Promise<Uint8Array> {
    return this.#connection.bytes(`/sandboxes/${this.sandboxId}/fs/read`, {
      signal: options.signal,
      query: this.#fsQuery(path),
    });
  }

  async listDir(path: string, options: CallOptions = {}): Promise<DirEntry[]> {
    const entries = await this.#connection.json<ApiPathDirectoryEntry[]>(
      "GET",
      `/sandboxes/${this.sandboxId}/fs/list`,
      { signal: options.signal, query: this.#fsQuery(path) },
    );
    return entries.map((entry) => ({
      name: entry.name,
      rawName: decodeHex(entry.raw_name),
      type: entry.type,
    }));
  }

  async stat(path: string, options: StatOptions = {}): Promise<FileStat> {
    const followSymlinks = options.followSymlinks ?? true;
    const status = await this.#connection.json<ApiFileStatus>(
      "GET",
      `/sandboxes/${this.sandboxId}/fs/stat`,
      { signal: options.signal, query: { ...this.#fsQuery(path), follow_symlinks: followSymlinks } },
    );
    return {
      type: status.type,
      mode: status.mode,
      uid: status.uid,
      gid: status.gid,
      size: status.size,
      links: status.links,
      mtime: status.mtime,
      ctime: status.ctime,
      target: status.target !== undefined ? decodeHex(status.target) : null,
      mount: status.mount ?? null,
    };
  }

  #fsQuery(path: string): Query {
    return { thread: this.thread, event: this.event, path };
  }
}

export async function seekPoint(
  connection: Connection,
  sandboxId: number,
  target: ApiSeekTarget,
  watch?: readonly WatchInput[],
  signal?: AbortSignal,
): Promise<SandboxPoint> {
  const body = { target, ...encodeWatches(watch) };
  const stop = await connection.json<ApiDvmStop>("POST", `/sandboxes/${sandboxId}/points/seek`, {
    signal,
    body,
  });
  return new SandboxPoint(connection, sandboxId, stop);
}

function encodeWatches(watch?: readonly WatchInput[]): { watch_points?: ApiWatchPoint[] } {
  if (watch === undefined) return {};
  return {
    watch_points: watch.map((item) => {
      if (typeof item !== "string") return item;
      const preset = NAMED_WATCH_POINTS.get(item);
      if (preset) return preset;
      if (item.startsWith("syscall:")) return { kind: "named_syscall", name: item.slice("syscall:".length) };
      throw new DvmError(`unknown watch point: ${JSON.stringify(item)}`);
    }),
  };
}

function parseStopEvent(at: ApiStopEvent): StopEvent {
  if ("syscall" in at) {
    return {
      kind: "syscall",
      syscall: at.syscall.number,
      name: at.syscall.name,
      args: at.syscall.args,
      decoded: (at.syscall.decoded ?? []).map(parseDecodedArgument),
    };
  }
  return {
    kind: "page_fault",
    address: at.page_fault.address,
    write: at.page_fault.write,
    present: at.page_fault.present,
  };
}

function parseDecodedArgument(item: ApiDecodedArgument): DecodedArgument {
  const truncated = item.truncated ?? false;
  if ("path" in item) return { arg: item.arg, kind: "path", value: item.path, truncated };
  if ("strings" in item) return { arg: item.arg, kind: "strings", value: item.strings, truncated };
  if ("address" in item) return { arg: item.arg, kind: "address", value: item.address, truncated };
  return { arg: item.arg, kind: "bytes", value: item.bytes, truncated };
}

function decodeHex(hex: string): Uint8Array {
  const bytes = new Uint8Array(hex.length / 2);
  for (let i = 0; i < hex.length; i += 2) {
    bytes[i / 2] = Number.parseInt(hex.slice(i, i + 2), 16);
  }
  return bytes;
}

function join(chunks: Uint8Array[], length: number): Uint8Array {
  if (chunks.length === 1) return chunks[0]!;
  const joined = new Uint8Array(length);
  let at = 0;
  for (const chunk of chunks) {
    joined.set(chunk, at);
    at += chunk.length;
  }
  return joined;
}
