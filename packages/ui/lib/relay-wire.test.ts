import { afterEach, expect, it, vi } from "vitest";
import type { DesktopApi } from "./desktop";
import { frameSockets } from "./relay-wire";

const hostId = "5b0e2c8e-6f1a-4c55-9a43-2f0d7b1e9c11";

// The desktop app's bridge (window.tjommie.remote), with its frame and closed events in the test's hands.
function bridge(ids: string[]) {
  const frames = new Set<(id: string, frame: Uint8Array) => void>(), closes = new Set<(id: string, code?: number) => void>();
  const sent: [string, Uint8Array][] = [];
  const remote: DesktopApi["remote"] = {
    open: vi.fn(async () => ({ id: ids.shift() ?? "none" })),
    send: async (id, frame) => { sent.push([id, frame]); },
    close: vi.fn(),
    onFrame: (listener) => { frames.add(listener); return () => { frames.delete(listener); }; },
    onClosed: (listener) => { closes.add(listener); return () => { closes.delete(listener); }; },
    seal: async () => {},
    read: async () => null,
    delete: async () => {},
  };
  vi.stubGlobal("window", { tjommie: { remote } });
  return { remote, sent, frame: (id: string, bytes: number[]) => { for (const listener of frames) listener(id, new Uint8Array(bytes)); }, closed: (id: string, code?: number) => { for (const listener of closes) listener(id, code); } };
}

afterEach(() => vi.unstubAllGlobals());

it("carries one wire's frames over the desktop app's bridge, in order, and passes on the relay's close code", async () => {
  const desktop = bridge(["w2"]);
  const endpoint = { kind: "relay", url: "https://relay.example.workers.dev", hostId } as const;
  const wire = await frameSockets().open(endpoint, 5_000);
  expect(desktop.remote.open).toHaveBeenCalledWith(endpoint, 5_000);
  const received: number[][] = [];
  wire.onFrame((frame) => received.push([...frame]));
  desktop.frame("w1", [9]); // another wire's
  desktop.frame("w2", [1, 2]);
  expect(received).toEqual([[1, 2]]);
  wire.send(new Uint8Array([3]));
  wire.send(new Uint8Array([4]));
  await vi.waitFor(() => expect(desktop.sent.map(([id, frame]) => [id, [...frame]])).toEqual([["w2", [3]], ["w2", [4]]]));
  const codes: (number | undefined)[] = [];
  wire.onClose((code) => codes.push(code));
  desktop.closed("w2", 4404);
  expect(codes).toEqual([4404]);
  expect(() => wire.send(new Uint8Array([5]))).toThrow();
});

it("fails the open of a wire that closed before the bridge said which one it was, keeping the relay's close code", async () => {
  const desktop = bridge(["w3"]);
  const open = frameSockets().open({ kind: "relay", url: "https://relay.example.workers.dev", hostId }, 5_000);
  desktop.closed("w3", 4404); // the relay had no PC for this leg, and said so before the bridge's reply arrived
  await expect(open).rejects.toMatchObject({ code: 4404 });
});
