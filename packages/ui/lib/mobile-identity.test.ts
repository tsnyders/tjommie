import { expect, it } from "vitest";
import { decodeMobileIdentity, encodeMobileIdentity } from "./mobile-identity";

it("binds the protected phone key to the selected host ID and pinned host key", () => {
  const id = "c28b37d4-3068-4071-8335-dd08eed89b37", hostKey = "AQ".padEnd(43, "A"), privateKey = new Uint8Array(32).fill(9);
  const envelope = encodeMobileIdentity(id, hostKey, privateKey);
  expect(decodeMobileIdentity(envelope, id, hostKey)).toEqual(privateKey);
  expect(() => decodeMobileIdentity(envelope, "d0facaa2-7ea3-49ec-9585-49df2a9a3582", hostKey)).toThrow("does not match");
  expect(() => decodeMobileIdentity(envelope, id, "Ag".padEnd(43, "A"))).toThrow("does not match");
  expect(() => decodeMobileIdentity(new Uint8Array(32), id, hostKey)).toThrow();
});
