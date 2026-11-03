import assert from "node:assert/strict";
import { createHash, randomUUID } from "node:crypto";
import { closeSync, mkdtempSync, openSync, readFileSync, readSync, rmSync, writeSync } from "node:fs";
import { open } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { EncryptedMessages } from "../../../packages/client/src/encrypted-channel.ts";
import { generateKeypair, initNoise, initiator, responder } from "../../../packages/transport/src/index.ts";
import ranges from "./range.cjs";

const CHUNK = 256 * 1024;
const SIZE = 200 * 1024 * 1024;
const root = mkdtempSync(join(tmpdir(), "tjommie-files-transfer-"));
const sessions = new Map();
let checks = 0;
const check = (run) => { run(); checks++; };
const context = { hostId: "synthetic-host", deviceId: "synthetic-device", sessionId: "synthetic-session", live: true, revoked: false };
const attachment = { hostId: context.hostId, messageId: "synthetic-message", attachmentId: "synthetic-attachment", revision: "", size: SIZE };
const path = join(root, "snapshot.bin");
const writeAll = (fd, bytes) => { for (let offset = 0; offset < bytes.length;) offset += writeSync(fd, bytes, offset); };
function diskDigest(file) {
  const fd = openSync(file, "r"), hash = createHash("sha256"), buffer = Buffer.alloc(CHUNK);
  try { for (;;) { const count = readSync(fd, buffer); if (!count) return hash.digest("hex"); hash.update(buffer.subarray(0, count)); } }
  finally { closeSync(fd); }
}

// This is an executable contract prototype, not the production authorization implementation.
async function transferOpen(who, ref) {
  assert(who.live && !who.revoked, "forbidden");
  assert.equal(who.hostId, attachment.hostId, "forbidden");
  assert.equal(ref.messageId, attachment.messageId, "unavailable");
  assert.equal(ref.attachmentId, attachment.attachmentId, "unavailable");
  assert([...sessions.values()].filter((entry) => entry.deviceId === who.deviceId).length < 2, "busy");
  const id = randomUUID();
  sessions.set(id, { ...who, fd: await open(path, "r"), reading: false });
  return id;
}
async function transferRead(who, id, offset, length, revision = attachment.revision) {
  const entry = sessions.get(id);
  assert(who.live && !who.revoked, "forbidden");
  assert(entry && entry.deviceId === who.deviceId && entry.sessionId === who.sessionId && entry.hostId === who.hostId, "forbidden");
  assert.equal(revision, attachment.revision, "expired");
  assert(Number.isSafeInteger(offset) && offset >= 0 && offset <= SIZE && Number.isSafeInteger(length) && length > 0 && length <= CHUNK, "bad_params");
  assert(!entry.reading, "busy");
  entry.reading = true;
  try {
    const bytes = Buffer.alloc(Math.min(length, SIZE - offset));
    const { bytesRead } = await entry.fd.read(bytes, 0, bytes.length, offset);
    assert.equal(bytesRead, bytes.length, "corrupt");
    assert(who.live && !who.revoked, "forbidden");
    return { transferId: id, revision, offset, bytes: bytes.toString("base64"), eof: offset + bytesRead === SIZE };
  } finally { entry.reading = false; }
}
async function transferClose(id) { const entry = sessions.get(id); sessions.delete(id); await entry?.fd.close(); }

try {
  const source = openSync(path, "wx");
  const bytes = Buffer.alloc(CHUNK);
  for (let index = 0; index < bytes.length; index++) bytes[index] = index % 251;
  const sourceHash = createHash("sha256");
  for (let offset = 0; offset < SIZE; offset += bytes.length) { writeAll(source, bytes); sourceHash.update(bytes); }
  closeSync(source);
  attachment.revision = sourceHash.digest("hex");
  initNoise(readFileSync(new URL("../../../packages/transport/wasm/tjommie_noise_bg.wasm", import.meta.url)));
  const host = generateKeypair(), device = generateKeypair();
  const client = initiator({ privateKey: device.privateKey, remotePublicKey: host.publicKey });
  const server = responder({ privateKey: host.privateKey });
  server.readHandshake(client.writeHandshake(new Uint8Array()));
  client.readHandshake(server.writeHandshake(new Uint8Array()));
  const sender = new EncryptedMessages(server), receiver = new EncryptedMessages(client);
  const id = await transferOpen(context, attachment);
  const other = await transferOpen(context, attachment);
  await assert.rejects(transferOpen(context, attachment), /busy/); checks++;
  await transferClose(other);
  for (const [who, ref] of [[{ ...context, hostId: "another-host" }, attachment], [context, { ...attachment, messageId: "missing" }], [context, { ...attachment, attachmentId: "missing" }]]) {
    await assert.rejects(transferOpen(who, ref)); checks++;
  }
  for (const [who, offset, length, revision] of [
    [{ ...context, sessionId: "another-session" }, 0, 1], [{ ...context, deviceId: "another-device" }, 0, 1],
    [{ ...context, hostId: "another-host" }, 0, 1], [{ ...context, revoked: true }, 0, 1], [{ ...context, live: false }, 0, 1],
    [context, -1, 1], [context, SIZE + 1, 1], [context, 0.5, 1], [context, 0, CHUNK + 1], [context, 0, 0], [context, 0, 1, "stale"],
  ]) { await assert.rejects(transferRead(who, id, offset, length, revision)); checks++; }
  const outstanding = transferRead(context, id, 0, CHUNK);
  await assert.rejects(transferRead(context, id, 0, 1), /busy/); checks++; await outstanding;
  check(() => assert.equal(ranges.httpRange("bytes=-9", 10).start, 1));
  check(() => assert.deepEqual(ranges.httpRange("bytes=8-99", 10), { start: 8, end: 9 }));
  check(() => assert.deepEqual(ranges.httpRange("bytes=0-", 10), { start: 0, end: 9 }));
  for (const header of ["bytes=9-1", "bytes=10-", "bytes=-0", "bytes=0-1,3-4", "bytes=0.5-2", "bytes=9007199254740992-"]) check(() => assert.throws(() => ranges.httpRange(header, 10), RangeError));
  const target = openSync(join(root, "download.part"), "wx");
  const destinationHash = createHash("sha256");
  const before = process.memoryUsage();
  const started = performance.now();
  let wireBytes = 0, jsonBytes = 0, maxFrames = 0, maxRss = before.rss, maxArrayBuffers = before.arrayBuffers, controlMessages = 0;
  for (let offset = 0; offset < SIZE; offset += CHUNK) {
    const result = await transferRead(context, id, offset, CHUNK);
    const json = JSON.stringify({ type: "res", id: 1, ok: true, result });
    jsonBytes += Buffer.byteLength(json);
    const frames = sender.encode(json);
    maxFrames = Math.max(maxFrames, frames.length);
    let decoded = null;
    for (const frame of frames) { wireBytes += frame.length + 4; decoded = receiver.receive(frame); }
    const answer = JSON.parse(decoded).result;
    assert.equal(answer.offset, offset); assert.equal(answer.revision, attachment.revision);
    const chunk = Buffer.from(answer.bytes, "base64");
    assert.equal(chunk.length, Math.min(CHUNK, SIZE - offset));
    assert.equal(chunk.toString("base64"), answer.bytes);
    destinationHash.update(chunk); writeAll(target, chunk);
    // Actual Noise-framed control messages interleave at RPC message boundaries.
    let control = null;
    for (const frame of sender.encode('{"type":"event","event":"synthetic-stop-ack","data":{}}')) control = receiver.receive(frame);
    assert.equal(JSON.parse(control).event, "synthetic-stop-ack"); controlMessages++;
    const memory = process.memoryUsage(); maxRss = Math.max(maxRss, memory.rss); maxArrayBuffers = Math.max(maxArrayBuffers, memory.arrayBuffers);
  }
  closeSync(target);
  check(() => assert.equal(destinationHash.digest("hex"), attachment.revision));
  check(() => assert.equal(diskDigest(join(root, "download.part")), attachment.revision));
  check(() => assert.equal(maxFrames, 6));
  check(() => assert.equal(controlMessages, SIZE / CHUNK));
  const eof = await transferRead(context, id, SIZE, 1);
  check(() => assert.equal(eof.bytes, "")); check(() => assert.equal(eof.eof, true));
  await transferClose(id);
  await assert.rejects(transferRead(context, id, 0, 1), /forbidden/); checks++;
  sender.close(); receiver.close(); host.privateKey.fill(0); device.privateKey.fill(0);
  console.log(JSON.stringify({ kind: "P0 files transfer", checks, rawBytes: SIZE, chunkBytes: CHUNK, chunks: SIZE / CHUNK, jsonBytes, wireBytesIncludingLengthPrefix: wireBytes, overheadPercent: (wireBytes / SIZE - 1) * 100, maxNoiseFramesPerChunk: maxFrames, elapsedMs: performance.now() - started, rssBefore: before.rss, rssPeak: maxRss, arrayBuffersBefore: before.arrayBuffers, arrayBuffersPeak: maxArrayBuffers, sha256: attachment.revision, controlMessages, boundary: "Actual Snow WASM + EncryptedMessages in one Node process; prototype authorization; not socket, renderer, or native-picker proof" }, null, 2));
} finally {
  for (const id of sessions.keys()) await transferClose(id);
  assert.equal(dirname(resolve(root)), resolve(tmpdir()));
  assert(root.startsWith(join(resolve(tmpdir()), "tjommie-files-transfer-")));
  rmSync(root, { recursive: true, force: true });
}
