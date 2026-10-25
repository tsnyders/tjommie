import { z } from "zod";
import { RemoteKey } from "@tjommie/protocol";

const Identity = z.object({ v: z.literal(1), hostId: z.uuid(), hostKey: RemoteKey, privateKey: RemoteKey }).strict();
export function encodeMobileIdentity(hostId: string, hostKey: string, privateKey: Uint8Array): Uint8Array {
  if (privateKey.length !== 32) throw new Error("The connection key is invalid.");
  const privateText = btoa(String.fromCharCode(...privateKey)).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
  return new TextEncoder().encode(JSON.stringify(Identity.parse({ v: 1, hostId, hostKey, privateKey: privateText })));
}
// The whole envelope is authenticated by Android Keystore AES-GCM. Plain host-list edits cannot
// change the pinned PC key while continuing to use this phone identity.
export function decodeMobileIdentity(bytes: Uint8Array, hostId: string, hostKey: string): Uint8Array {
  try {
    const saved = Identity.parse(JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes)));
    if (saved.hostId !== hostId || saved.hostKey !== hostKey) throw new Error();
    return Uint8Array.from(atob(saved.privateKey.replace(/-/g, "+").replace(/_/g, "/") + "="), (value) => value.charCodeAt(0));
  } catch { throw new Error("The saved PC identity does not match its protected connection key. Forget this PC and pair again."); }
}
