/** The connection to dvm.run's API: its address, the key every request
 * bears, and what a refusal means. */

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
const RETRY_DELAYS_MS: readonly number[] = [100, 250, 500, 1_000];
const MAX_RETRY_AFTER_MS = 5_000;
const CONNECT_ERROR_CODES = new Set([
  "ECONNREFUSED",
  "ENOTFOUND",
  "EAI_AGAIN",
  "EHOSTUNREACH",
  "ENETUNREACH",
  "ETIMEDOUT",
  "UND_ERR_CONNECT_TIMEOUT",
  "EPROTO",
]);

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

export type Sleep = (ms: number, signal?: AbortSignal) => Promise<void>;

interface Request extends CallOptions {
  query?: Query;
  idempotent?: boolean;
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
  readonly #sleep: Sleep;

  constructor(
    apiUrl: string,
    key?: string,
    fetcher: typeof fetch = fetch,
    sleep: Sleep = defaultSleep,
  ) {
    this.#api = `${apiUrl}/api`;
    this.#key = key;
    this.#fetch = fetcher;
    this.#sleep = sleep;
  }

  /** The connection `options` say, the key found as `ClientOptions` says. */
  static async open(
    options: ClientOptions = {},
    fetcher: typeof fetch = fetch,
    sleep: Sleep = defaultSleep,
  ): Promise<Connection> {
    const url = apiUrlOf(options.apiUrl);
    const key = options.apiKey || environment("DVM_API_KEY") || (await savedKeyFor(url));
    if (!key) {
      throw new DvmError(
        `no API key for ${url}: run \`dvm-run login\`, pass apiKey, or set DVM_API_KEY`,
      );
    }
    return new Connection(url, key, fetcher, sleep);
  }

  /** The answer's JSON; null for an empty one. */
  async json<T>(method: string, path: string, request: Request & { body?: unknown } = {}): Promise<T> {
    const body = request.body === undefined ? undefined : JSON.stringify(request.body);
    const idempotent = request.idempotent ?? (method === "GET" || method === "PUT" || method === "DELETE");
    const bytes = await this.#send(method, path, request, idempotent, body);
    const text = new TextDecoder().decode(bytes);
    return (text ? JSON.parse(text) : null) as T;
  }

  async input<T>(path: string, body: Uint8Array, request: Request): Promise<T> {
    const [bytes, attempt, status, statusText] = await this.#exchange("POST", path, request, true, body);
    if (status < 200 || status >= 300) {
      const refusal = refusalFromBytes(status, statusText, bytes);
      const query = request.query ?? {};
      if (
        attempt > 0
        && query.end === true
        && body.byteLength === 0
        && refusal instanceof ConflictError
        && refusal.reason === `input already ended at length ${query.offset}`
      ) {
        return { length: Number(query.offset), ended: true } as T;
      }
      throw refusal;
    }
    return JSON.parse(new TextDecoder().decode(bytes)) as T;
  }

  async bytes(path: string, request: Request = {}): Promise<Uint8Array> {
    return this.#send("GET", path, request, true);
  }

  async #send(
    method: string,
    path: string,
    request: Request,
    idempotent: boolean,
    body?: string | Uint8Array,
  ): Promise<Uint8Array> {
    const [bytes, , status, statusText] = await this.#exchange(method, path, request, idempotent, body);
    if (status < 200 || status >= 300) throw refusalFromBytes(status, statusText, bytes);
    return bytes;
  }

  async #exchange(
    method: string,
    path: string,
    request: Request,
    idempotent: boolean,
    body?: string | Uint8Array,
  ): Promise<readonly [Uint8Array, number, number, string]> {
    const url = new URL(this.#api + path);
    for (const [name, value] of Object.entries(request.query ?? {})) {
      url.searchParams.set(name, String(value));
    }
    const headers: Record<string, string> = {};
    if (this.#key) headers.authorization = `Bearer ${this.#key}`;
    if (typeof body === "string") headers["content-type"] = "application/json";
    if (body instanceof Uint8Array) headers["content-type"] = "application/octet-stream";

    const delays: readonly (number | undefined)[] = [...RETRY_DELAYS_MS, undefined];
    for (let attempt = 0; attempt < delays.length; attempt++) {
      const delay = delays[attempt];
      request.signal?.throwIfAborted();
      const signal = withTimeout(request.signal, QUIET_LIMIT_MS);
      let response: Response;
      try {
        response = await this.#fetch(url, {
          method,
          headers,
          body: body instanceof Uint8Array ? new Uint8Array(body) : body,
          signal,
        });
      } catch (error) {
        if (request.signal?.aborted || error instanceof DvmError) throw error;
        if (delay !== undefined && retryableError(error, idempotent, true)) {
          await this.#sleep(delay, request.signal);
          continue;
        }
        throw this.#networkError(error);
      }

      let bytes: Uint8Array;
      try {
        bytes = new Uint8Array(await response.arrayBuffer());
      } catch (error) {
        if (request.signal?.aborted || error instanceof DvmError) throw error;
        if (delay !== undefined && retryableError(error, idempotent, false)) {
          await this.#sleep(delay, request.signal);
          continue;
        }
        throw this.#networkError(error);
      }

      if (!response.ok && delay !== undefined && retryableStatus(response.status, bytes, idempotent)) {
        await this.#sleep(retryDelayMs(response.headers.get("retry-after"), delay), request.signal);
        continue;
      }
      return [bytes, attempt, response.status, response.statusText];
    }
    throw new DvmError("unreachable");
  }

  #networkError(error: unknown): NetworkError {
    const reason = error instanceof Error && error.cause instanceof Error ? error.cause : error;
    return new NetworkError(`couldn't reach ${this.#api}: ${String(reason)}`, { cause: error });
  }
}

function retryableError(error: unknown, idempotent: boolean, sending: boolean): boolean {
  if (idempotent) return true;
  return sending && isConnectError(error);
}

function isConnectError(error: unknown): boolean {
  const code = causeCode(error);
  if (!code) return false;
  return (
    CONNECT_ERROR_CODES.has(code)
    || code.startsWith("ERR_SSL_")
    || code.startsWith("ERR_TLS_")
    || code.startsWith("CERT_")
  );
}

function causeCode(error: unknown): string | undefined {
  let current = error;
  for (let depth = 0; depth < 4 && current instanceof Error; depth++) {
    const code = (current as { code?: unknown }).code;
    if (typeof code === "string") return code;
    current = current.cause;
  }
  return undefined;
}

function retryableStatus(status: number, bytes: Uint8Array, idempotent: boolean): boolean {
  if (status === 408 || status === 429) return true;
  if (!idempotent) return false;
  if (status === 502 || status === 504) return true;
  return status === 503 && !isControllerRefusal(bytes);
}

function isControllerRefusal(bytes: Uint8Array): boolean {
  try {
    const parsed: unknown = JSON.parse(new TextDecoder().decode(bytes));
    return Boolean(parsed && typeof parsed === "object" && "error" in parsed && typeof parsed.error === "string");
  } catch {
    return false;
  }
}

function retryDelayMs(header: string | null, fallback: number): number {
  const trimmed = header?.trim() ?? "";
  if (/^\d+$/.test(trimmed)) {
    return Math.min(MAX_RETRY_AFTER_MS, Math.max(fallback, Number(trimmed) * 1_000));
  }
  return fallback;
}

/** The error a refusal is: its status's, with the controller's reason. */
export async function refusalOf(response: Response): Promise<ApiError> {
  const bytes = new Uint8Array(await response.arrayBuffer().catch(() => new ArrayBuffer(0)));
  return refusalFromBytes(response.status, response.statusText, bytes);
}

function refusalFromBytes(status: number, statusText: string, bytes: Uint8Array): ApiError {
  const text = new TextDecoder().decode(bytes);
  let reason = text || statusText;
  try {
    const answer: unknown = JSON.parse(text);
    if (answer && typeof answer === "object" && "message" in answer && typeof answer.message === "string") {
      reason = answer.message;
    }
  } catch {
    // Not JSON: the body as it is.
  }
  const Refusal = REFUSALS.get(status) ?? ApiError;
  return new Refusal(status, reason);
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

function defaultSleep(ms: number, signal?: AbortSignal): Promise<void> {
  signal?.throwIfAborted();
  return new Promise<void>((resolve, reject) => {
    const onAbort = () => {
      clearTimeout(timer);
      reject(signal!.reason);
    };
    const timer = setTimeout(() => {
      signal?.removeEventListener("abort", onAbort);
      resolve();
    }, ms);
    signal?.addEventListener("abort", onAbort, { once: true });
  });
}
