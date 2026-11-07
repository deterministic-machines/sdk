/** Environment variables for one test's while, then as they were. */

export async function withEnvironment(
  changes: Record<string, string | undefined>,
  run: () => Promise<void>,
): Promise<void> {
  const before = Object.fromEntries(Object.keys(changes).map((name) => [name, process.env[name]]));
  setEnvironment(changes);
  try {
    await run();
  } finally {
    setEnvironment(before);
  }
}

function setEnvironment(values: Record<string, string | undefined>): void {
  for (const [name, value] of Object.entries(values)) {
    if (value === undefined) delete process.env[name];
    else process.env[name] = value;
  }
}
