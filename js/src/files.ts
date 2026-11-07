/** Files in a sandbox, written in and read out whole. */

import type { CallOptions, Query } from "./connection.ts";
import type { SandboxCalls } from "./sandbox-calls.ts";

/** A sandbox's files: `sandbox.files`. */
export class Files {
  readonly #calls: SandboxCalls;

  constructor(calls: SandboxCalls) {
    this.#calls = calls;
  }

  /** Writes `data` to the file at `path`, in place of what it held; a new
   * file gets `mode` (`0o755`), else `0o644`. */
  async write(
    path: string,
    data: string | Uint8Array,
    options: CallOptions & { mode?: number } = {},
  ): Promise<void> {
    const query: Query = { path };
    if (options.mode !== undefined) query.mode = options.mode.toString(8);
    const content = typeof data === "string" ? new TextEncoder().encode(data) : data;
    const calls = this.#calls;
    await calls.whileLive(() =>
      calls.connection.bytes("PUT", `${calls.path}/files`, { query, content, signal: options.signal }),
    );
  }

  /** The file at `path`, as it is. */
  read(path: string, options: CallOptions = {}): Promise<Uint8Array> {
    const calls = this.#calls;
    return calls.whileLive(() =>
      calls.connection.bytes("GET", `${calls.path}/files`, { query: { path }, signal: options.signal }),
    );
  }

  /** The file at `path`, as UTF-8 text. */
  async readText(path: string, options: CallOptions = {}): Promise<string> {
    return new TextDecoder().decode(await this.read(path, options));
  }
}
