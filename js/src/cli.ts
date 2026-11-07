#!/usr/bin/env node
/** `dvm-run`, the SDK's command line, as the Python SDK's: `npx dvm-run
 * login`. */

import { parseArgs } from "node:util";

import { DEFAULT_API_URL } from "./connection.ts";
import { DvmError } from "./errors.ts";
import { login } from "./login.ts";

const USAGE = `usage: dvm-run login [--api-url URL] [--no-browser]

Get an API key for this machine, approved in your browser, and save it where
the SDK finds it.

  --api-url URL  the dvm.run to log in to: DVM_API_URL, else ${DEFAULT_API_URL}
  --no-browser   only print the link to approve the login at`;

async function main(args: string[]): Promise<number> {
  let parsed;
  try {
    parsed = parseArgs({
      args,
      allowPositionals: true,
      options: {
        "api-url": { type: "string" },
        "no-browser": { type: "boolean" },
        help: { type: "boolean", short: "h" },
      },
    });
  } catch (error) {
    console.error(`dvm-run: ${(error as Error).message}\n\n${USAGE}`);
    return 2;
  }
  const { values, positionals } = parsed;
  if (values.help) {
    console.log(USAGE);
    return 0;
  }
  if (positionals.length !== 1 || positionals[0] !== "login") {
    console.error(USAGE);
    return 2;
  }
  try {
    await login({ apiUrl: values["api-url"], openBrowser: !values["no-browser"] });
  } catch (error) {
    if (!(error instanceof DvmError)) throw error;
    console.error(`dvm-run login: ${error.message}`);
    return 1;
  }
  return 0;
}

process.exitCode = await main(process.argv.slice(2));
