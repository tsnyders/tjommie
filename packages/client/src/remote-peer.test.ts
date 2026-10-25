import { readFileSync } from "node:fs";
import { RemoteConfirm, RemotePreface, remotePrologue } from "@tjommie/protocol";
import { beforeAll, expect, it } from "vitest";
import { generateKeypair, initNoise, responder, type NoiseSession } from "../../transport/src/index.ts";
import { EncryptedMessages } from "./encrypted-channel.ts";
import { authenticateRemote, RemoteRefusedError } from "./remote-peer.ts";
import type { FrameTransport } from "./frame-socket.ts";

const hostId = "99cf98ba-2638-4b16-8a6e-1691c1673b45";
const deviceId = "04364399-a4af-4ba6-a1e0-58ee51a40822";
const preface = { v: 1, hostId, mode: "resume" } as const;

beforeAll(() => initNoise(new Uint8Array(readFileSync(new URL("../../transport/wasm/tjommie_noise_bg.wasm", import.meta.url)))));

function exchange(response: unknown) {
  const hostKeys = generateKeypair(), deviceKeys = generateKeypair();
  const frames = new Set<(frame: Uint8Array) => void>(), closes = new Set<() => void>();
  let phase: "preface" | "handshake" | "confirm" | "done" = "preface", noise: NoiseSession | undefined, messages: EncryptedMessages | undefined;
  let closed = false;
  const deliver = (frame: Uint8Array): void => { for (const listener of frames) listener(frame); };
  const wire: FrameTransport = {
    send: (frame) => {
      if (phase === "preface") {
        expect(RemotePreface.parse(JSON.parse(new TextDecoder().decode(frame)))).toEqual(preface);
        noise = responder({ privateKey: hostKeys.privateKey, prologue: remotePrologue(preface) });
        phase = "handshake"; return;
      }
      if (phase === "handshake") {
        noise!.readHandshake(frame);
        const reply = noise!.writeHandshake(new Uint8Array());
        messages = new EncryptedMessages(noise!); phase = "confirm"; deliver(reply); return;
      }
      if (phase === "confirm") {
        const text = messages!.receive(frame);
        if (text === null) return;
        RemoteConfirm.parse(JSON.parse(text));
        phase = "done";
        for (const reply of messages!.encode(JSON.stringify(response))) deliver(reply);
      }
    },
    close: () => { if (closed) return; closed = true; messages?.close(); noise?.close(); for (const listener of closes) listener(); },
    onFrame: (listener) => { frames.add(listener); return () => { frames.delete(listener); }; },
    onClose: (listener) => { closes.add(listener); return () => { closes.delete(listener); }; },
  };
  return authenticateRemote({ wire, preface, privateKey: deviceKeys.privateKey, hostPublicKey: hostKeys.publicKey, name: "Phone" });
}

it("returns the relay URL from an authenticated ready message", async () => {
  const peer = await exchange({ type: "ready", deviceId, hostId, hostName: "My PC", relay: "https://r.example.workers.dev" });
  expect(peer.relay).toBe("https://r.example.workers.dev");
  peer.transport.close();
});

it.each(["revoked", "unknown"] as const)("distinguishes an authenticated %s refusal from network loss", async (reason) => {
  const failure = exchange({ type: "refused", reason });
  await expect(failure).rejects.toBeInstanceOf(RemoteRefusedError);
  await expect(failure).rejects.toMatchObject({ reason });
});

it("keeps a transport failure distinct from an authenticated refusal", async () => {
  const hostKeys = generateKeypair(), deviceKeys = generateKeypair();
  const wire: FrameTransport = {
    send: () => { throw new Error("Synthetic network loss"); }, close: () => {},
    onFrame: () => () => {}, onClose: () => () => {},
  };
  const failure = authenticateRemote({ wire, preface, privateKey: deviceKeys.privateKey, hostPublicKey: hostKeys.publicKey, name: "Phone" });
  await expect(failure).rejects.toThrow("The PC could not be authenticated.");
  await expect(failure).rejects.not.toBeInstanceOf(RemoteRefusedError);
});
