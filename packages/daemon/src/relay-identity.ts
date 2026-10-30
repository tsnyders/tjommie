import { createHash, createPrivateKey, createPublicKey, generateKeyPairSync, sign, timingSafeEqual } from "node:crypto";
import { resolve } from "node:path";
import { RemoteKey } from "@tjommie/protocol";
import { z } from "zod";
import { windowsCredentialTransport, type CredentialTransport } from "./account-credentials.ts";

const StoredKey = z.object({ v: z.literal(1), privateKey: z.string().regex(/^[A-Za-z0-9_-]{64,512}$/), publicKey: RemoteKey }).strict();
export type RelayIdentity = { publicKey: Uint8Array; sign(message: Uint8Array): Uint8Array };

export async function loadRelayIdentity(options: { root: string; credentials?: CredentialTransport }): Promise<RelayIdentity> {
  const base = resolve(options.root);
  const namespace = createHash("sha256").update(process.platform === "win32" ? base.toLowerCase() : base).digest("hex").slice(0, 32);
  const target = `Tjommie/${namespace}/relay-login`;
  const vault = options.credentials ?? windowsCredentialTransport;
  try {
    let saved = await vault({ operation: "read", target });
    if (saved === null) {
      const pair = generateKeyPairSync("ed25519");
      const privateKey = pair.privateKey.export({ format: "der", type: "pkcs8" });
      try {
        const publicKey = pair.publicKey.export({ format: "der", type: "spki" }).subarray(-32);
        saved = JSON.stringify(StoredKey.parse({ v: 1, privateKey: privateKey.toString("base64url"), publicKey: publicKey.toString("base64url") }));
        await vault({ operation: "write", target, key: saved });
      } finally { privateKey.fill(0); }
    }
    const record = StoredKey.parse(JSON.parse(saved));
    const bytes = Buffer.from(record.privateKey, "base64url");
    let key;
    try { key = createPrivateKey({ key: bytes, format: "der", type: "pkcs8" }); }
    finally { bytes.fill(0); }
    if (key.asymmetricKeyType !== "ed25519") throw new Error();
    const publicKey = createPublicKey(key).export({ format: "der", type: "spki" }).subarray(-32);
    if (!timingSafeEqual(publicKey, Buffer.from(record.publicKey, "base64url"))) throw new Error();
    return { publicKey: new Uint8Array(publicKey), sign: (message) => new Uint8Array(sign(null, message, key)) };
  } catch {
    throw new Error("Tjommie could not load this PC's relay connection key from Windows Credential Manager. No replacement identity was created.");
  }
}
