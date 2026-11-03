import { expect, it } from "vitest";
import { HostCheckRuntime } from "./project-checks.ts";
import { tempRoot } from "./testing.ts";

it("runs an explicit argument array with a scrubbed environment and bounded output", async () => {
  const result = await new HostCheckRuntime().run({ id: "check", label: "Synthetic check", command: process.execPath,
    args: ["-e", "if(process.env.NODE_OPTIONS || process.env.ANTHROPIC_API_KEY)process.exit(3);process.stdout.write('x'.repeat(300000))"], timeoutMs: 2000 }, tempRoot(), new AbortController().signal);
  expect(result.exitCode).toBe(0); expect(result.truncated).toBe(true); expect(result.output.length).toBeLessThanOrEqual(262_144);
});

it("never runs an already-cancelled check and reports a missing executable without echoing arguments", async () => {
  const controller = new AbortController(); controller.abort();
  const check = { id: "check", label: "Synthetic check", command: "tjommie-missing-synthetic-check", args: ["synthetic-private-argument"], timeoutMs: 1000 };
  await expect(new HostCheckRuntime().run(check, tempRoot(), controller.signal)).rejects.toThrow(/cancelled/);
  await expect(new HostCheckRuntime().run(check, tempRoot(), new AbortController().signal)).rejects.toThrow("The configured check could not start.");
});
