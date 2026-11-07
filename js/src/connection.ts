/** The connection to dvm.run's API: its address, the key every request
 * bears, what a refusal means, and server-sent events. */

import {
  ApiError,
  AuthenticationError,
  ConflictError,
  DvmError,
  InvalidRequestError,
  NotFoundError,
} from "./errors.ts";

export const DEFAULT_API_URL = "https://sandbox.dvm.run";

// The controller answers a `?newer_than=` read within 30 s, and keeps a quiet
// event stream alive with comments more often than this.
const QUIET_LIMIT_MS = 60_000;

/** Which dvm.run, and as whom. */
export interface ClientOptions {
  /** Else `DVM_API_KEY`, else the key `dvm-run login` saved for `apiUrl`. */
  apiKey?: string;
  /** Else `DVM_API_URL`, else `https://sandbox.dvm.run`. */
  apiUrl?: string;
}

/** What every call takes. */
export interface CallOptions {
  signal?: AbortSignal;
}

export type Query = Record<string, string | number | boolean>;

interface Request extends CallOptions {
  query?: Query;
}

/** dvm.run couldn't be reached, or the connection to it dropped. */
export class NetworkError extends DvmError {}

const REFUSALS = new Map<number, typeof ApiError>([
  [400, InvalidRequestError],
  [401, AuthenticationError],
  [404, NotFoundError],
  [409, ConflictError],
]);

/** dvm.run's API, at an address, as the user whose key it bears; with no
 * key, as no one, for what needs none (a login). */
export class Connection {
  readonly #api: string;
  readonly #key: string | undefined;
  readonly #fetch: typeof fetch;

  constructor(apiUrl: string, key?: string, fetcher: typeof fetch = fetch) {
    this.#api = `${apiUrl}/api`;
    this.#key = key;
    this.#fetch = fetcher;
  }

  /** The connection `options` say, the key found as `ClientOptions` says. */
  static async open(options: ClientOptions = {}, fetcher: typeof fetch = fetch): Promise<Connection> {
    const url = apiUrlOf(options.apiUrl);
    const key = options.apiKey || environment("DVM_API_KEY") || (await savedKeyFor(url));
    if (!key) {
      throw new DvmError(
        `no API key for ${url}: run \`dvm-run login\`, pass apiKey, or set DVM_API_KEY`,
      );
    }
    return new Connection(url, key, fetcher);
  }

  /** The answer's JSON; null for an empty one. */
  async json<T>(method: string, path: string, request: Request & { body?: unknown } = {}): Promise<T> {
    const body = request.body === undefined ? undefined : JSON.stringify(request.body);
    const signal = withTimeout(request.signal, QUIET_LIMIT_MS);
    const response = await this.#send(method, path, request, body, signal);
    const text = await this.#read(() => response.text(), request.signal);
    return (text ? JSON.parse(text) : null) as T;
  }

  /** The answer's body, as it is. */
  async bytes(
    method: string,
    path: string,
    request: Request & { content?: Uint8Array } = {},
  ): Promise<Uint8Array> {
    const response = await this.#send(method, path, request, request.content);
    return new Uint8Array(await this.#read(() => response.arrayBuffer(), request.signal));
  }

  /** Each server-sent event's data, as JSON, until the stream ends. A
   * stream that goes quiet for longer than the controller lets one is
   * dropped, as `NetworkError`. */
  async *events(path: string, request: Request = {}): AsyncGenerator<unknown> {
    const quiet = new AbortController();
    const dropQuiet = () => quiet.abort(new NetworkError(`${path}'s stream went quiet`));
    let timer = setTimeout(dropQuiet, QUIET_LIMIT_MS);
    const signal = request.signal ? AbortSignal.any([request.signal, quiet.signal]) : quiet.signal;
    let reader: ReadableStreamDefaultReader<string> | undefined;
    try {
      const response = await this.#send("GET", path, request, undefined, signal);
      reader = response.body!.pipeThrough(new TextDecoderStream()).getReader();
      let pending = "";
      let data: string[] = [];
      for (;;) {
        const read = await this.#read(() => reader!.read(), request.signal);
        if (read.done) return;
        clearTimeout(timer);
        timer = setTimeout(dropQuiet, QUIET_LIMIT_MS);
        pending += read.value;
        let end: number;
        while ((end = pending.indexOf("\n")) >= 0) {
          const line = pending.slice(0, end).replace(/\r$/, "");
          pending = pending.slice(end + 1);
          if (line.startsWith("data:")) {
            data.push(line.slice(5).replace(/^ /, ""));
          } else if (line === "" && data.length > 0) {
            yield JSON.parse(data.join("\n"));
            data = [];
          }
        }
      }
    } finally {
      clearTimeout(timer);
      await reader?.cancel().catch(() => undefined);
    }
  }

  /** The response, refused unless it's a success; `signal` is the call's,
   * and what else may end it. */
  async #send(
    method: string,
    path: string,
    request: Request,
    body?: string | Uint8Array,
    signal = request.signal,
  ): Promise<Response> {
    const url = new URL(this.#api + path);
    for (const [name, value] of Object.entries(request.query ?? {})) {
      url.searchParams.set(name, String(value));
    }
    const headers: Record<string, string> = {};
    if (this.#key) headers.authorization = `Bearer ${this.#key}`;
    if (typeof body === "string") headers["content-type"] = "application/json";
    const response = await this.#read(
      () => this.#fetch(url, { method, headers, body, signal }),
      request.signal,
    );
    if (!response.ok) throw await refusalOf(response);
    return response;
  }

  /** What `read` gives, its failure a `NetworkError` unless the call was
   * aborted, or the failure is one already. */
  async #read<T>(read: () => Promise<T>, aborted?: AbortSignal): Promise<T> {
    try {
      return await read();
    } catch (error) {
      if (aborted?.aborted || error instanceof DvmError) throw error;
      const reason = error instanceof Error && error.cause instanceof Error ? error.cause : error;
      throw new NetworkError(`couldn't reach ${this.#api}: ${String(reason)}`, { cause: error });
    }
  }
}

/** The error a refusal is: its status's, with the controller's reason. */
export async function refusalOf(response: Response): Promise<ApiError> {
  const text = await response.text().catch(() => "");
  let reason = text || response.statusText;
  try {
    const answer: unknown = JSON.parse(text);
    if (answer && typeof answer === "object" && "message" in answer && typeof answer.message === "string") {
      reason = answer.message;
    }
  } catch {
    // Not JSON: the body as it is.
  }
  const Refusal = REFUSALS.get(response.status) ?? ApiError;
  return new Refusal(response.status, reason);
}

/** dvm.run's address: `given`, else `DVM_API_URL`, else dvm.run's. */
export function apiUrlOf(given?: string): string {
  return (given || environment("DVM_API_URL") || DEFAULT_API_URL).replace(/\/+$/, "");
}

function environment(name: string): string | undefined {
  return typeof process === "undefined" ? undefined : process.env[name];
}

/** The saved key, read only now: a runtime with no file system works given
 * a key. */
async function savedKeyFor(apiUrl: string): Promise<string | undefined> {
  const { savedKey } = await import("./credentials.ts");
  return savedKey(apiUrl);
}

/** `signal`, and a timeout after `ms`. */
function withTimeout(signal: AbortSignal | undefined, ms: number): AbortSignal {
  const timeout = AbortSignal.timeout(ms);
  return signal ? AbortSignal.any([signal, timeout]) : timeout;
}
