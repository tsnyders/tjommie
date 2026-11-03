import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, readdirSync, renameSync, rmdirSync, unlinkSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import type { AttachmentInfo } from "../../protocol/src/attachments.ts";
import { tempRoot } from "../../daemon/src/testing.ts";
import { AttachmentSaves, decodeAttachmentChunk } from "./attachment-save.ts";

const saves = new AttachmentSaves();
afterEach(async () => { await saves.cancelOwner("owner"); });
const info = (bytes: Buffer): AttachmentInfo => ({ id: "41f8bac7-358c-48cf-9f48-a3a358804da6", hostId: "host", name: "file.bin", size: bytes.length, contentType: "application/octet-stream", sha256: createHash("sha256").update(bytes).digest("hex") });
describe("current-device staged downloads", () => {
  it("renames only after count/digest verification and preserves a destination on cancellation/corruption", async () => {
    const root = tempRoot(), destination = join(root, "download.bin"), bytes = Buffer.from("verified bytes"); writeFileSync(destination, "existing");
    const first = await saves.start("owner", info(bytes), destination);
    await saves.append("owner", first.saveId, 0, bytes.toString("base64")); expect(readFileSync(destination, "utf8")).toBe("existing");
    expect(await saves.finish("owner", first.saveId)).toEqual({ status: "saved" }); expect(readFileSync(destination)).toEqual(bytes);
    const cancelled = await saves.start("owner", info(bytes), destination); await saves.append("owner", cancelled.saveId, 0, Buffer.from("some").toString("base64"));
    expect(await saves.cancel("owner", cancelled.saveId)).toEqual({ status: "cancelled" }); expect(readFileSync(destination)).toEqual(bytes);
    const corrupt = await saves.start("owner", { ...info(bytes), sha256: "0".repeat(64) }, destination); await saves.append("owner", corrupt.saveId, 0, bytes.toString("base64"));
    await expect(saves.finish("owner", corrupt.saveId)).rejects.toThrow(/content check/); expect(readFileSync(destination)).toEqual(bytes);
    expect(readdirSync(root)).toEqual(["download.bin"]);
  });
  it("rejects a late/wrong-owner/out-of-order/malformed write and never commits an incomplete save", async () => {
    const root = tempRoot(), destination = join(root, "download.bin"), bytes = Buffer.from("bytes"), first = await saves.start("owner", info(bytes), destination);
    await expect(saves.append("another", first.saveId, 0, bytes.toString("base64"))).rejects.toThrow(/available/);
    await expect(saves.append("owner", first.saveId, 1, bytes.toString("base64"))).rejects.toThrow(/sequence/);
    await expect(saves.append("owner", first.saveId, 0, "AAAA\n")).rejects.toThrow(/Invalid/);
    await expect(saves.finish("owner", first.saveId)).rejects.toThrow(/content check/); expect(existsSync(destination)).toBe(false);
    await expect(saves.append("owner", first.saveId, 0, bytes.toString("base64"))).rejects.toThrow(/available/);
    expect(readdirSync(root)).toEqual([]);
  });
  it("bounds pending opens, releases on host abort, and accepts a verified empty file", async () => {
    const root = tempRoot(), bytes = Buffer.from("abc"), abort = new AbortController();
    const pending = [saves.start("owner", info(bytes), join(root, "a"), abort.signal), saves.start("owner", info(bytes), join(root, "b"), abort.signal)];
    await expect(saves.start("owner", info(bytes), join(root, "c"))).rejects.toThrow(/Two downloads/); await Promise.all(pending);
    abort.abort(); await saves.cancelOwner("owner"); expect(readdirSync(root)).toEqual([]);
    const empty = await saves.start("owner", info(Buffer.alloc(0)), join(root, "empty")); await saves.finish("owner", empty.saveId); expect(readFileSync(join(root, "empty"))).toHaveLength(0);
  });
  it("rejects noncanonical/oversized base64 before native decoding", () => {
    for (const encoded of ["", "abc", "Zh==", "A".repeat(349532), "AA==\n"]) expect(() => decodeAttachmentChunk(encoded)).toThrow(/Invalid/);
  });
  it("waits for pending native opens to remove their private partials on invalidation", async () => {
    const root = tempRoot(), opening = saves.start("owner", info(Buffer.from("abc")), join(root, "cancelled")), cancel = saves.cancelOwner("owner");
    await expect(opening).rejects.toThrow(/closed/); await cancel; expect(readdirSync(root)).toEqual([]);
  });
  it("retains failed cleanup for retry instead of claiming a later owner clear succeeded", async () => {
    const root = tempRoot(), first = await saves.start("owner", info(Buffer.from("abc")), join(root, "destination"));
    const partial = join(root, readdirSync(root)[0]), held = `${partial}.held`;
    renameSync(partial, held); mkdirSync(partial); writeFileSync(join(partial, "synthetic-blocker"), "block");
    await expect(saves.cancel("owner", first.saveId)).rejects.toThrow(); await expect(saves.cancelOwner("owner")).rejects.toThrow();
    unlinkSync(join(partial, "synthetic-blocker")); rmdirSync(partial); writeFileSync(partial, "retry the same managed path");
    await saves.cancelOwner("owner"); expect(readdirSync(root)).toHaveLength(1); expect(existsSync(held)).toBe(true); expect(existsSync(partial)).toBe(false); unlinkSync(held);
  });
});
