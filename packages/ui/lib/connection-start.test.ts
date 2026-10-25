import type { Client } from "@tjommie/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { DesktopConnection } from "./desktop";

const connect = vi.hoisted(() => vi.fn<typeof import("@tjommie/client").connect>());
vi.mock("@tjommie/client", async (importOriginal) => ({ ...(await importOriginal<typeof import("@tjommie/client")>()), connect }));

let pushConnection: (connection: DesktopConnection) => void = () => {};

beforeEach(() => {
  vi.resetModules(); // each test gets a fresh connection module, store and client classes
  vi.useFakeTimers();
  connect.mockReset();
  vi.stubGlobal("window", {
    tjommie: {
      connection: async () => ({ ok: true, port: 47100, token: "first" }),
      onConnection: (listener: (connection: DesktopConnection) => void) => {
        pushConnection = listener;
        return () => {};
      },
      onDaemonFailed: () => () => {},
      restartDaemon: async () => {},
    },
  });
});

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

// Starts the UI's connection with fresh modules. Imported after the reset, so the errors thrown here are the
// classes connection.ts sees.
async function start() {
  const client = await import("@tjommie/client");
  const { startConnection } = await import("./connection");
  const { getState } = await import("./store");
  // A daemon client whose list calls answer, or fail with the daemon's reason.
  const daemonClient = (lists: "answer" | "fail"): Client => {
    const answers: Record<string, unknown> = { "bots.list": [], "settings.get": { chainLimit: 5 }, "routines.list": [], "rooms.list": [], "setup.status": [] };
    const call = async (method: string): Promise<unknown> => {
      if (lists === "fail" && method !== "setup.status") throw new client.ClientRpcError("internal", "The database is locked.");
      return answers[method];
    };
    return { call: call as unknown as Client["call"], events: [], subscribe: () => () => {}, disconnected: new Promise(() => {}), close: vi.fn(async () => {}) };
  };
  return { client, daemonClient, startConnection, status: () => getState().connection };
}

describe("the first connection", () => {
  it("says why after a few failed tries, and keeps trying until the daemon answers", async () => {
    const { daemonClient, startConnection, status } = await start();
    connect.mockImplementation(async () => daemonClient("fail"));
    startConnection();
    await vi.advanceTimersByTimeAsync(0);
    expect(status()).toEqual({ status: "connecting" });
    await vi.advanceTimersByTimeAsync(2_000);
    expect(status()).toEqual({ status: "connecting" });
    await vi.advanceTimersByTimeAsync(2_000);
    expect(connect).toHaveBeenCalledTimes(3);
    expect(status()).toEqual({ status: "failed", reason: "The database is locked." });

    connect.mockImplementation(async () => daemonClient("answer"));
    await vi.advanceTimersByTimeAsync(2_000);
    expect(status()).toEqual({ status: "ready" });
  });

  it("says in plain words when the daemon can't be reached", async () => {
    const { client, startConnection, status } = await start();
    connect.mockImplementation(async () => {
      throw new client.ClientConnectionError(1006, "", false);
    });
    startConnection();
    await vi.advanceTimersByTimeAsync(4_000);
    expect(connect).toHaveBeenCalledTimes(3);
    expect(status()).toEqual({ status: "failed", reason: "Tjommie couldn't reach its background service." });
  });
});

describe("a pushed connection", () => {
  it("shows Reconnecting while it replaces a live one", async () => {
    const { daemonClient, startConnection, status } = await start();
    const first = daemonClient("answer");
    connect.mockImplementationOnce(async () => first);
    startConnection();
    await vi.advanceTimersByTimeAsync(0);
    expect(status()).toEqual({ status: "ready" });

    let open: (client: Client) => void = () => {};
    connect.mockImplementationOnce(() => new Promise<Client>((resolve) => (open = resolve)));
    pushConnection({ port: 47101, token: "second" });
    await vi.advanceTimersByTimeAsync(0);
    expect(status()).toEqual({ status: "reconnecting" });
    expect(first.close).toHaveBeenCalled();

    open(daemonClient("answer"));
    await vi.advanceTimersByTimeAsync(0);
    expect(status()).toEqual({ status: "ready" });
  });
});
