import type { Client } from "@tjommie/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

const HOME = "5b0e2c8e-6f1a-4c55-9a43-2f0d7b1e9c11";

// A PC whose startup lists answer at once: the desktop's own daemon and the saved one alike.
const pc = (): Client => {
  const answers: Record<string, unknown> = { "settings.get": { chainLimit: 5, relayUrl: null } };
  const call = async (method: string): Promise<unknown> => answers[method] ?? [];
  return { call: call as unknown as Client["call"], events: [], subscribe: () => () => {}, disconnected: new Promise(() => {}), close: async () => {} };
};
const saved = vi.hoisted(() => ({ refusal: null as Error | null, forget: vi.fn<(id: string) => Promise<void>>(async () => {}), local: vi.fn(), remote: vi.fn(), invalidate: vi.fn<() => Promise<void>>(async () => {}) }));
vi.mock("./attachment-device", () => ({ attachmentDevice: () => ({ invalidate: saved.invalidate }) }));
vi.mock("@tjommie/client", async (original) => ({ ...(await original<typeof import("@tjommie/client")>()), connect: async () => { saved.local(); return pc(); } }));
vi.mock("./remote-hosts", async (original) => ({
  ...(await original<typeof import("./remote-hosts")>()),
  isAndroidApp: () => false,
  remoteHosts: () => [{ id: HOME, name: "Home PC" }],
  openRemoteClient: async () => {
    saved.remote();
    if (saved.refusal) throw saved.refusal;
    return pc();
  },
  forgetRemoteHost: saved.forget,
}));

const settle = async (): Promise<void> => { for (let turn = 0; turn < 10; turn += 1) await vi.advanceTimersByTimeAsync(0); };

beforeEach(() => {
  vi.resetModules();
  vi.useFakeTimers();
  saved.refusal = null;
  saved.forget.mockClear();
  saved.local.mockClear();
  saved.remote.mockClear();
  saved.invalidate.mockReset().mockResolvedValue();
  vi.stubGlobal("window", {
    tjommie: { connection: async () => ({ ok: true, port: 47100, token: "local" }), onConnection: () => () => {}, onDaemonFailed: () => () => {}, restartDaemon: async () => {} },
  });
});
afterEach(() => { vi.useRealTimers(); vi.unstubAllGlobals(); });

it.each(["forget", "refused"] as const)("hard-cancels a byte-free picker on %s instead of rebinding after reconnect", async (reason) => {
  const connection = await import("./connection"); connection.startConnection(); await settle(); connection.connectRemote(HOME); await settle();
  const abort = new AbortController(), context = connection.captureAttachmentDownload(abort), guard = connection.captureAttachmentSave()!;
  if (reason === "forget") await connection.forgetRemote(HOME);
  else { const { RemoteRefusedError } = await import("../../client/src/remote-peer.ts"); saved.refusal = new RemoteRefusedError("revoked"); connection.connectRemote(HOME); }
  await settle(); expect(abort.signal.aborted).toBe(true); expect(guard.current()).toBe(false); expect(saved.invalidate).toHaveBeenCalledWith(undefined); context.close();
});

it("closes previous host file handles before a new host becomes ready", async () => {
  const { startConnection, connectRemote } = await import("./connection");
  const { getState } = await import("./store");
  startConnection(); await settle();
  let finish = () => {};
  saved.invalidate.mockImplementationOnce(() => new Promise<void>((resolve) => { finish = resolve; }));
  connectRemote(HOME); await settle();
  expect(saved.remote).not.toHaveBeenCalled();
  expect(getState().connection.status).not.toBe("ready");
  finish(); await settle();
  expect(saved.remote).toHaveBeenCalledOnce();
  expect(getState()).toMatchObject({ connection: { status: "ready" }, host: HOME });
});

it("remembers which saved PC the desktop window is on, from the moment it's picked until This PC or Forget", async () => {
  const { connectLocal, connectRemote, forgetRemote, startConnection } = await import("./connection");
  const { getState } = await import("./store");
  startConnection();
  await settle();
  expect(getState()).toMatchObject({ connection: { status: "ready" }, host: null });

  connectRemote(HOME);
  expect(getState().host).toBe(HOME); // at once, so the menu button names it while it connects
  await settle();
  expect(getState()).toMatchObject({ connection: { status: "ready" }, host: HOME });

  connectLocal();
  expect(getState().host).toBeNull();
  await settle();
  expect(getState()).toMatchObject({ connection: { status: "ready" }, host: null });

  connectRemote(HOME);
  await settle();
  await forgetRemote(HOME);
  expect(getState().host).toBeNull();
});

it("forgets a PC that removed this device only once its key is gone: a failed delete leaves the removed screen, the PC and its key as they were", async () => {
  const { connectRemote, forgetRemote, startConnection } = await import("./connection");
  const { getState } = await import("./store");
  const { RemoteRefusedError } = await import("../../client/src/remote-peer.ts");
  startConnection();
  await settle();
  saved.refusal = new RemoteRefusedError("revoked");
  connectRemote(HOME);
  await settle();
  expect(getState()).toMatchObject({ connection: { status: "removed" }, host: HOME });
  saved.local.mockClear();

  saved.forget.mockRejectedValueOnce(new Error("Windows couldn't delete this PC's key."));
  await expect(forgetRemote(HOME)).rejects.toThrow("Windows couldn't delete this PC's key.");
  await settle();
  expect(getState()).toMatchObject({ connection: { status: "removed" }, host: HOME }); // still on the removed screen, which says why
  expect(saved.local).not.toHaveBeenCalled();

  await forgetRemote(HOME); // the delete works this time: this PC's own Tjommie comes back
  await settle();
  expect(saved.forget).toHaveBeenLastCalledWith(HOME);
  expect(getState()).toMatchObject({ connection: { status: "ready" }, host: null });
  expect(saved.local).toHaveBeenCalledTimes(1);
});
