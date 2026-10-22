import { EventEmitter } from "node:events";
import { PassThrough } from "node:stream";
import { execFileSync, spawn, type ChildProcess } from "node:child_process";
import { afterEach, describe, expect, it, vi } from "vitest";
import { startDaemonProcess } from "./daemon-process.ts";

vi.mock("node:child_process", () => ({ execFileSync: vi.fn(), spawn: vi.fn() }));

function fakeChild(): { child: ChildProcess; stdout: PassThrough } {
  const child = Object.assign(new EventEmitter(), {
    stdin: new PassThrough(),
    stdout: new PassThrough(),
    stderr: new PassThrough(),
    pid: 4242,
    exitCode: null,
    signalCode: null,
    kill: vi.fn(() => true),
  }) as unknown as ChildProcess;
  return { child, stdout: child.stdout as PassThrough };
}

function launch(): { execPath: string; args: string[]; env: NodeJS.ProcessEnv } {
  return { execPath: "node", args: [], env: {} };
}

afterEach(() => {
  vi.useRealTimers();
  vi.clearAllMocks();
});

describe("daemon process tree kill", () => {
  it("kills the daemon tree when graceful shutdown reaches its timeout", async () => {
    vi.useFakeTimers();
    const fake = fakeChild();
    vi.mocked(spawn).mockReturnValue(fake.child as ReturnType<typeof spawn>);
    const starting = startDaemonProcess(launch());
    fake.stdout.write("Tjommie daemon listening on http://127.0.0.1:47123\n");
    const daemon = await starting;

    const stopping = daemon.stop();
    await vi.advanceTimersByTimeAsync(10_000);
    if (process.platform === "win32") {
      expect(execFileSync).toHaveBeenCalledWith("taskkill", ["/PID", "4242", "/T", "/F"], { stdio: "ignore", windowsHide: true });
    } else {
      expect(fake.child.kill).toHaveBeenCalledWith("SIGKILL");
    }

    fake.child.emit("close", 1, null);
    await stopping;
  });

  it("kills the daemon tree when startup times out", async () => {
    vi.useFakeTimers();
    const fake = fakeChild();
    vi.mocked(spawn).mockReturnValue(fake.child as ReturnType<typeof spawn>);
    const starting = startDaemonProcess(launch(), () => {}, 100);
    const rejected = expect(starting).rejects.toThrow("Tjommie's background service didn't start in time.");

    await vi.advanceTimersByTimeAsync(100);
    await rejected;
    if (process.platform === "win32") {
      expect(execFileSync).toHaveBeenCalledWith("taskkill", ["/PID", "4242", "/T", "/F"], { stdio: "ignore", windowsHide: true });
    } else {
      expect(fake.child.kill).toHaveBeenCalledWith("SIGKILL");
    }

    fake.child.emit("close", null, "SIGKILL");
  });

  it("does not taskkill a child that exited before startup failure", async () => {
    const fake = fakeChild();
    vi.mocked(spawn).mockReturnValue(fake.child as ReturnType<typeof spawn>);
    const starting = startDaemonProcess(launch());
    const rejected = expect(starting).rejects.toThrow("Tjommie's background service stopped (exit code 1).");
    Object.assign(fake.child, { exitCode: 1 });
    fake.child.emit("close", 1, null);

    await rejected;
    expect(execFileSync).not.toHaveBeenCalled();
    expect(fake.child.kill).not.toHaveBeenCalled();
  });
});
