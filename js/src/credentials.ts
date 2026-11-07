/** The API keys `dvm-run login` saved, by the address of the dvm.run each is
 * for: `dvm/credentials.json` in `$XDG_CONFIG_HOME`, else in `~/.config`,
 * readable by its user alone. The Python SDK reads and writes the same
 * file. */

import { mkdir, open, readFile, rename, rm } from "node:fs/promises";
import { homedir } from "node:os";
import { dirname, join } from "node:path";

import { DvmError } from "./errors.ts";

export function credentialsPath(): string {
  const config = process.env.XDG_CONFIG_HOME || join(homedir(), ".config");
  return join(config, "dvm", "credentials.json");
}

/** The key saved for the dvm.run at `apiUrl`, if there's one. */
export async function savedKey(apiUrl: string): Promise<string | undefined> {
  const keys = await savedKeys(credentialsPath());
  const address = addressOf(apiUrl);
  return Object.hasOwn(keys, address) ? keys[address] : undefined;
}

/** Saves `key` for the dvm.run at `apiUrl`, in place of the one saved for it
 * before; the file it's in. */
export async function saveKey(apiUrl: string, key: string): Promise<string> {
  const path = credentialsPath();
  const keys = await savedKeys(path);
  keys[addressOf(apiUrl)] = key;
  await mkdir(dirname(path), { recursive: true, mode: 0o700 });
  // Written beside it, then moved over it, so a reader never sees half of
  // it; made for its user alone before the key is in it.
  const written = `${path}.new`;
  await rm(written, { force: true });
  const file = await open(written, "wx", 0o600);
  try {
    await file.writeFile(`${JSON.stringify({ keys }, null, 2)}\n`);
  } finally {
    await file.close();
  }
  await rename(written, path);
  return path;
}

async function savedKeys(path: string): Promise<Record<string, string>> {
  let text: string;
  try {
    text = await readFile(path, "utf8");
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return {};
    throw error;
  }
  try {
    const keys: unknown = JSON.parse(text).keys;
    if (
      keys !== null &&
      typeof keys === "object" &&
      !Array.isArray(keys) &&
      Object.values(keys).every((key) => typeof key === "string")
    ) {
      return keys as Record<string, string>;
    }
  } catch {
    // Not JSON: refused below, as a file of another shape is.
  }
  throw new DvmError(`${path} isn't what \`dvm-run login\` writes: delete it, and log in again`);
}

function addressOf(apiUrl: string): string {
  return apiUrl.replace(/\/+$/, "");
}
