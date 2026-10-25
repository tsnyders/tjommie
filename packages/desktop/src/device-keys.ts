import { createHash } from "node:crypto";
import { resolve } from "node:path";
import { windowsCredentialTransport, type CredentialTransport } from "../../daemon/src/account-credentials.ts";

const targetOf = (namespace: string, name: unknown): string => {
  if (typeof name !== "string" || !/^[a-z0-9._-]{1,64}$/.test(name)) throw new Error("Invalid device key.");
  return `Tjommie/${namespace}/device/${name}`;
};
const storageError = () => new Error("Device key storage is unavailable.");

export function createDeviceKeys(dataRoot: string, transport: CredentialTransport = windowsCredentialTransport) {
  const namespace = createHash("sha256").update(resolve(dataRoot).toLowerCase()).digest("hex").slice(0, 24);
  return {
    async seal(name: unknown, value: unknown): Promise<void> {
      const target = targetOf(namespace, name);
      // 1920 raw bytes encode to the credential transport's 2560-byte limit.
      if (!(value instanceof Uint8Array) || value.length === 0 || value.length > 1920) throw new Error("Invalid device key.");
      const bytes = Buffer.from(value);
      try { await transport({ operation: "write", target, key: bytes.toString("base64url") }); }
      catch { throw storageError(); }
      finally { bytes.fill(0); }
    },
    async read(name: unknown): Promise<Uint8Array | null> {
      const target = targetOf(namespace, name);
      try {
        const saved = await transport({ operation: "read", target });
        if (saved === null) return null;
        if (!/^[A-Za-z0-9_-]{1,2560}$/.test(saved)) throw storageError();
        const bytes = Buffer.from(saved, "base64url");
        try {
          if (bytes.length === 0 || bytes.length > 1920 || bytes.toString("base64url") !== saved) throw storageError();
          return Uint8Array.from(bytes);
        } finally { bytes.fill(0); }
      } catch { throw storageError(); }
    },
    async delete(name: unknown): Promise<void> {
      const target = targetOf(namespace, name);
      try { await transport({ operation: "delete", target }); } catch { throw storageError(); }
    },
  };
}
