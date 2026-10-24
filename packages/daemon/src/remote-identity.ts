import { createHash } from "node:crypto";
import { resolve } from "node:path";
import { z } from "zod";
import { RemoteKey, RemoteName } from "@tjommie/protocol";
import { windowsCredentialTransport, type CredentialTransport } from "./account-credentials.ts";
import { newId, nowIso, type Db } from "./db.ts";

const StoredIdentity = z.object({ v: z.literal(1), id: z.uuid(), privateKey: RemoteKey, publicKey: RemoteKey }).strict();
const HostRow = z.object({ id: z.uuid(), name: RemoteName, publicKey: RemoteKey });
export type HostIdentity = { id: string; name: string; publicKey: Uint8Array; privateKey: Uint8Array };

export async function loadRemoteIdentity(options: {
  db: Db; root: string; name: string;
  generate: () => { privateKey: Uint8Array; publicKey: Uint8Array };
  credentials?: CredentialTransport;
}): Promise<HostIdentity> {
  const base = resolve(options.root);
  const namespace = createHash("sha256").update(process.platform === "win32" ? base.toLowerCase() : base).digest("hex").slice(0, 32);
  const target = `Tjommie/${namespace}/remote-host`;
  const vault = options.credentials ?? windowsCredentialTransport;
  try {
    const row = options.db.prepare("SELECT id, name, public_key AS publicKey FROM remote_host WHERE singleton = 1").get();
    const saved = await vault({ operation: "read", target });
    let identity: z.infer<typeof StoredIdentity>;
    if (saved !== null) {
      identity = StoredIdentity.parse(JSON.parse(saved));
      if (row) {
        const host = HostRow.parse(row);
        if (host.id !== identity.id || host.publicKey !== identity.publicKey) throw new Error();
      }
    } else {
      if (row || options.db.prepare("SELECT id FROM devices LIMIT 1").get()) throw new Error();
      const keys = options.generate();
      try {
        if (keys.privateKey.length !== 32 || keys.publicKey.length !== 32) throw new Error();
        identity = StoredIdentity.parse({ v: 1, id: newId(), privateKey: Buffer.from(keys.privateKey).toString("base64url"), publicKey: Buffer.from(keys.publicKey).toString("base64url") });
        await vault({ operation: "write", target, key: JSON.stringify(identity) });
      } finally { keys.privateKey.fill(0); }
    }
    const name = row ? HostRow.parse(row).name : RemoteName.parse(options.name);
    if (!row) options.db.prepare("INSERT INTO remote_host(singleton,id,name,public_key,created_at) VALUES(1,?,?,?,?)").run(identity.id, name, identity.publicKey, nowIso());
    return { id: identity.id, name, privateKey: new Uint8Array(Buffer.from(identity.privateKey, "base64url")), publicKey: new Uint8Array(Buffer.from(identity.publicKey, "base64url")) };
  } catch { throw new Error("Tjommie could not load this PC's connection key from Windows Credential Manager. Existing devices have not been given a replacement identity."); }
}
