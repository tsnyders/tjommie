import { EventEmitter } from "node:events";
import { PassThrough } from "node:stream";
import type { ChildProcessWithoutNullStreams } from "node:child_process";
import { expect, it, vi } from "vitest";
import { CheckTerminationError, HostCheckRuntime } from "./project-checks.ts";

const fixture = vi.hoisted(() => ({ launch: vi.fn(), stop: vi.fn() }));
vi.mock("./cli.ts", async (original) => ({ ...await original<typeof import("./cli.ts")>(), launchCli: fixture.launch, killTreeAndWait: fixture.stop }));

it("reports uncertain Stop promptly even when the owned check never closes", async () => {
  const child = Object.assign(new EventEmitter(), { pid: 4242420, stdin: new PassThrough(), stdout: new PassThrough(), stderr: new PassThrough() });
  fixture.launch.mockResolvedValue(child as unknown as ChildProcessWithoutNullStreams);
  fixture.stop.mockRejectedValue(new Error("Synthetic termination refusal"));
  const controller = new AbortController();
  const run = new HostCheckRuntime().run({ id: "check", label: "Synthetic check", command: "synthetic-no-executable", args: [], timeoutMs: 60000 }, ".", controller.signal);
  const reported = expect(run).rejects.toBeInstanceOf(CheckTerminationError);
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    await vi.waitFor(() => expect(child.stdin.writableEnded).toBe(true));
    controller.abort();
    await Promise.race([reported, new Promise<never>((_resolve, reject) => { timer = setTimeout(() => reject(new Error("Owned check did not report uncertain termination.")), 500); })]);
  } finally { clearTimeout(timer); child.emit("close", 0); child.stdin.destroy(); child.stdout.destroy(); child.stderr.destroy(); fixture.launch.mockReset(); fixture.stop.mockReset(); }
});
