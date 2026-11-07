/** `dvm-run login` and the key it saves, against answers of the test's own. */

import assert from "node:assert/strict";
import { mkdtemp, mkdir, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { test } from "node:test";

import { Connection } from "../src/connection.ts";
import { credentialsPath } from "../src/credentials.ts";
import { DvmError, NotFoundError } from "../src/errors.ts";
import { login } from "../src/login.ts";
import { withEnvironment } from "./environment.ts";

const URL_ = "http://dvm.test";
const SECRET = `dvm_${"a".repeat(64)}`;

/** Each test with a config directory of its own, and no key in the
 * environment. */
function loginTest(name: string, run: () => Promise<void>): void {
  test(name, async () => {
    const config = await mkdtemp(join(tmpdir(), "dvm-login-"));
    try {
      await withEnvironment(
        { XDG_CONFIG_HOME: config, DVM_API_KEY: undefined, DVM_API_URL: undefined },
        run,
      );
    } finally {
      await rm(config, { recursive: true, force: true });
    }
  });
}

/** Answers a login as dvm.run does, approved after the terminal asked for
 * its key `approvedAfter` times. */
function controller(approvedAfter: number, secret = SECRET) {
  const asked: Request[] = [];
  const fetcher: typeof fetch = async (input, init) => {
    const request = new Request(input, init);
    const path = new URL(request.url).pathname;
    if (path === "/api/cli-logins") {
      assert.ok(((await request.json()) as { machine: string }).machine);
      const started = {
        approve_url: "http://dvm.test/cli-login/page",
        code: "BCDF-GHJK",
        collect_token: "collect",
        expires_at: 0,
      };
      return Response.json(started, { status: 201 });
    }
    if (path === "/api/cli-logins/key") {
      assert.deepEqual(await request.json(), { collect_token: "collect" });
      asked.push(request);
      if (asked.length <= approvedAfter) return Response.json(null);
      const key = { id: 1, name: "Login on laptop", ending: secret.slice(-4) };
      return Response.json({ key, secret });
    }
    if (path === "/api/me") {
      assert.equal(request.headers.get("authorization"), `Bearer ${secret}`);
      return Response.json({ login: "alice", admin: false, limits: null });
    }
    throw new Error(path);
  };
  return { fetcher, asked };
}

/** The key the SDK's requests to `apiUrl` bear. */
async function keySentTo(apiUrl: string, apiKey?: string): Promise<string> {
  const sent: string[] = [];
  const fetcher: typeof fetch = async (input, init) => {
    const authorization = new Request(input, init).headers.get("authorization") ?? "";
    sent.push(authorization.replace(/^Bearer /, ""));
    return Response.json(null);
  };
  const connection = await Connection.open({ apiUrl, apiKey }, fetcher);
  await connection.json("GET", "/me");
  return sent[0]!;
}

const quiet = () => {};

loginTest("a login saves its key where the SDK finds it", async () => {
  const { fetcher, asked } = controller(2);
  const said: string[] = [];
  const path = await login({
    apiUrl: `${URL_}/`,
    openBrowser: false,
    say: (line) => said.push(line),
    fetch: fetcher,
  });
  assert.equal(asked.length, 3);
  assert.equal(said[0], "Approve this login in your browser: http://dvm.test/cli-login/page");
  assert.match(said[1]!, /BCDF-GHJK/);
  assert.match(said[2]!, /as alice/);
  assert.equal(path, credentialsPath());
  assert.equal((await stat(path)).mode & 0o777, 0o600);
  assert.equal(await keySentTo(URL_), SECRET);
});

loginTest("each dvm.run keeps its own key", async () => {
  await login({ apiUrl: URL_, openBrowser: false, say: quiet, fetch: controller(0).fetcher });
  const other = "http://other.test";
  await login({ apiUrl: other, openBrowser: false, say: quiet, fetch: controller(0, "dvm_b").fetcher });
  assert.equal(await keySentTo(URL_), SECRET);
  assert.equal(await keySentTo(other), "dvm_b");
  await assert.rejects(keySentTo("http://third.test"), /no API key for http:\/\/third\.test: run `dvm-run login`/);
});

loginTest("a key given or in the environment comes before the saved one", async () => {
  await login({ apiUrl: URL_, openBrowser: false, say: quiet, fetch: controller(0).fetcher });
  assert.equal(await keySentTo(URL_, "dvm_given"), "dvm_given");
  await withEnvironment({ DVM_API_KEY: "dvm_environment" }, async () => {
    assert.equal(await keySentTo(URL_), "dvm_environment");
  });
});

loginTest("an expired login says to log in again", async () => {
  const message = "no such login: it expired, or was used; run `dvm-run login` again";
  const fetcher: typeof fetch = async (input, init) => {
    if (new URL(new Request(input, init).url).pathname === "/api/cli-logins") {
      const started = { approve_url: "u", code: "c", collect_token: "t", expires_at: 0 };
      return Response.json(started, { status: 201 });
    }
    return Response.json({ message }, { status: 404 });
  };
  await assert.rejects(login({ apiUrl: URL_, openBrowser: false, say: quiet, fetch: fetcher }), (error) => {
    assert.ok(error instanceof NotFoundError);
    assert.match(error.message, /run `dvm-run login` again/);
    return true;
  });
  await assert.rejects(keySentTo(URL_), /no API key/);
});

loginTest("a credentials file it didn't write is refused", async () => {
  await mkdir(dirname(credentialsPath()), { recursive: true });
  await writeFile(credentialsPath(), "not json");
  await assert.rejects(keySentTo(URL_), (error) => {
    assert.ok(error instanceof DvmError);
    assert.match(error.message, /delete it, and log in again/);
    return true;
  });
});

loginTest("the Python SDK's file is the same", async () => {
  await mkdir(dirname(credentialsPath()), { recursive: true });
  // As Python's json.dump(indent=2) writes it.
  await writeFile(credentialsPath(), `{\n  "keys": {\n    "${URL_}": "dvm_python"\n  }\n}\n`);
  assert.equal(await keySentTo(URL_), "dvm_python");
});
