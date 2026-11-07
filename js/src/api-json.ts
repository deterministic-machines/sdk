/** The API's JSON, as far as the SDK reads it, snake_case as the controller
 * writes it. dvm-platform's tests run this SDK's tests against a controller,
 * which keeps the two in step. */

import type { ApiRunState } from "./run-state.ts";

export interface ApiRun {
  id: number;
  state: ApiRunState;
  version: number;
  runner: string | null;
  log_bytes: number | null;
  verdict: ApiVerdict | null;
}

export interface ApiSpec {
  name: string;
  image: string;
  entrypoint: string | null;
  command: string[];
  environment: { name: string; value: string }[];
  cpus: number;
  memory_mib: number;
  seed: number;
}

export interface ApiSandbox {
  id: number;
  spec: ApiSpec;
  run: ApiRun;
}

export interface ApiExecution extends ApiSandbox {
  commands: ApiCommand[];
  replays: ApiRun[];
}

export type ApiCommandKind = "starting" | "running" | "exited" | "refused" | "unfinished";

export interface ApiCommand {
  number: number;
  command: string | string[];
  argv: string[] | null;
  state: { kind: ApiCommandKind; code?: number; signal?: number; reason?: string };
  started_at: number | null;
  ended_at: number | null;
}

export type ApiOutputEvent =
  | { kind: "output"; stream: "stdout" | "stderr"; offset: number; chunk: string }
  | { kind: "command"; command: ApiCommand };

export type ApiVerdictKind = "same" | "other_exit" | "other_output" | "other_command";

export interface ApiVerdict {
  kind: ApiVerdictKind;
  command?: number;
  stream?: "stdout" | "stderr";
  at?: number;
  recorded?: number;
  replayed?: number;
}

export type ApiMoment =
  | { kind: "event"; thread: number; operation: number }
  | { kind: "time"; at_us: number };
