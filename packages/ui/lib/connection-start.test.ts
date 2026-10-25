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
  const { request, startConnection } = await import("./connection");
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
  return { client, daemonClient, request, startConnection, status: () => getState().connection };
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

describe("calls in flight", () => {
  it("keeps at most 16 calls outstanding, so a burst can't trip the phone host's limit of 32, and answers every call", async () => {
    const { daemonClient, request, startConnection, status } = await start();
    const answering = daemonClient("answer");
    const lists = answering.call as unknown as (method: string) => Promise<unknown>;
    let outstanding = 0;
    let most = 0;
    const ends: (() => void)[] = [];
    // A room's history waits until the test lets it answer; the startup lists answer at once.
    const call = async (method: string): Promise<unknown> => {
      if (method !== "room.history") return lists(method);
      outstanding += 1;
      most = Math.max(most, outstanding);
      await new Promise<void>((resolve) => ends.push(resolve));
      outstanding -= 1;
      return { messages: [], hasMore: false };
    };
    connect.mockImplementation(async () => ({ ...answering, call: call as unknown as Client["call"] }));
    startConnection();
    await vi.advanceTimersByTimeAsync(0);
    expect(status()).toEqual({ status: "ready" });

    let answered = 0;
    const burst = Array.from({ length: 40 }, () => request("room.history", { roomId: "team", limit: 10 }).then(() => (answered += 1)));
    while (answered < 40) {
      await vi.advanceTimersByTimeAsync(0);
      expect(outstanding).toBeLessThanOrEqual(16);
      for (const end of ends.splice(0)) end();
    }
    await Promise.all(burst);
    expect(most).toBe(16);
  });
});
