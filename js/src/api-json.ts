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
  replays: ApiRun[];
}

export type ApiVerdictKind = "same" | "other_exit" | "other_output";

export interface ApiVerdict {
  kind: ApiVerdictKind;
  stream?: "stdout" | "stderr";
  at?: number;
  recorded?: number;
  replayed?: number;
}
