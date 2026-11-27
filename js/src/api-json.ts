/** The API's JSON, as far as the SDK reads it, snake_case as the controller
 * writes it. dvm-platform's tests run this SDK's tests against a controller,
 * which keeps the two in step. */

import type { ApiRunState } from "./run-state.ts";

export interface ApiRun {
  id: number;
  state: ApiRunState;
  version: number;
  runner: string | null;
  has_timeline: boolean;
  inspectable: boolean;
  outdated: boolean;
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
  network?: { mode: string; allow?: string[] };
}

export interface ApiSandbox {
  id: number;
  spec: ApiSpec;
  run: ApiRun;
}

export type ApiWatchPoint =
  | { kind: "syscall" }
  | { kind: "named_syscall"; name: string }
  | { kind: "page_fault" }
  | { kind: "processes" }
  | { kind: "file_syscalls" }
  | { kind: "socket_syscalls" };

export type ApiSeekTarget =
  | { kind: "end" }
  | { kind: "at_ns"; ns: number }
  | { kind: "event"; thread: number; event: number };

export type ApiDecodedArgument =
  | { arg: number; path: string; truncated?: boolean }
  | { arg: number; strings: string[]; truncated?: boolean }
  | { arg: number; address: string; truncated?: boolean }
  | { arg: number; bytes: string; truncated?: boolean };

export type ApiStopEvent =
  | {
    syscall: {
      number: number;
      name: string;
      args: string[];
      decoded?: ApiDecodedArgument[];
    };
  }
  | {
    page_fault: {
      address: string;
      write: boolean;
      present: boolean;
    };
  };

export interface ApiDvmStop {
  thread: number;
  process: number;
  operations: number;
  event: number;
  recording_ns: number | null;
  at: ApiStopEvent;
  stats: Record<string, number>;
}

export interface ApiPathDirectoryEntry {
  name: string;
  raw_name: string;
  type: string;
}

export interface ApiFileStatus {
  type: string;
  mode: number;
  uid: number;
  gid: number;
  size: number;
  links: number;
  mtime: [number, number];
  ctime: [number, number];
  target?: string;
  mount?: string;
}
