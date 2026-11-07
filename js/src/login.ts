/** Logging in from a terminal (`dvm-run login`): dvm.run makes an API key
 * for the user who approves the login in their browser, and the terminal
 * saves it where the SDK finds it, so no one copies a key. The page that
 * approves it opens here; over SSH, the user opens the link it prints
 * wherever they like. The Python SDK's `dvm-run login` is the same. */

import { spawn } from "node:child_process";
import { hostname } from "node:os";

import { apiUrlOf, Connection } from "./connection.ts";
import { saveKey } from "./credentials.ts";

export interface LoginOptions {
  /** Else `DVM_API_URL`, else `https://sandbox.dvm.run`. */
  apiUrl?: string;
  /** Opens the page that approves it; else it's only printed. */
  openBrowser?: boolean;
  /** Says what to do, and what it did. */
  say?: (line: string) => void;
  fetch?: typeof fetch;
}

interface StartedLogin {
  approve_url: string;
  code: string;
  collect_token: string;
}

interface CreatedKey {
  key: { name: string };
  secret: string;
}

/** Logs this machine in, once its user approves the login in their
 * browser: the file its key is saved in. */
export async function login(options: LoginOptions = {}): Promise<string> {
  const url = apiUrlOf(options.apiUrl);
  const say = options.say ?? ((line: string) => console.log(line));
  const terminal = new Connection(url, undefined, options.fetch);
  const started = await terminal.json<StartedLogin>("POST", "/cli-logins", {
    body: { machine: hostname() },
  });
  say(`Approve this login in your browser: ${started.approve_url}`);
  say(`The page shows the code ${started.code}.`);
  if (options.openBrowser ?? true) openInBrowser(started.approve_url);
  const collect = { collect_token: started.collect_token };
  let created: CreatedKey | null = null;
  while (created === null) {
    created = await terminal.json<CreatedKey | null>("POST", "/cli-logins/key", { body: collect });
  }
  const user = new Connection(url, created.secret, options.fetch);
  const me = await user.json<{ login: string }>("GET", "/me");
  const path = await saveKey(url, created.secret);
  say(`Logged in to ${url} as ${me.login}, with the key ${created.key.name}.`);
  say(`It's saved in ${path}.`);
  return path;
}

function openInBrowser(url: string): void {
  const [command, ...args] =
    process.platform === "darwin"
      ? ["open", url]
      : process.platform === "win32"
        ? ["cmd", "/c", "start", "", url]
        : ["xdg-open", url];
  const opener = spawn(command!, args, { stdio: "ignore", detached: true });
  // With no browser here, the printed link is the way.
  opener.on("error", () => undefined);
  opener.unref();
}
