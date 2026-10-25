export interface ChannelCipher {
  encrypt(plaintext: Uint8Array): Uint8Array;
  decrypt(ciphertext: Uint8Array): Uint8Array;
  close(): void;
}
export const MAX_MESSAGE_BYTES = 16 * 1024 * 1024;
export const MAX_NOISE_FRAME = 65535;
const CHUNK_BYTES = 65519 - 5;

// Framing only: Snow owns authentication, encryption and ordered nonces. A message cannot interleave
// with another message; all fragments are emitted synchronously in one ordered send operation.
export class EncryptedMessages {
  #cipher: ChannelCipher;
  #buffer: Uint8Array | null = null;
  #at = 0;
  #closed = false;
  constructor(cipher: ChannelCipher) { this.#cipher = cipher; }
  encode(text: string): Uint8Array[] {
    try {
      if (this.#closed) throw new Error();
      const bytes = new TextEncoder().encode(text);
      if (bytes.length === 0 || bytes.length > MAX_MESSAGE_BYTES) throw new Error();
      const frames: Uint8Array[] = [];
      for (let at = 0; at < bytes.length; at += CHUNK_BYTES) {
        const chunk = bytes.subarray(at, at + CHUNK_BYTES);
        const payload = new Uint8Array(chunk.length + 5);
        payload[0] = at === 0 ? 1 : 0;
        new DataView(payload.buffer).setUint32(1, at === 0 ? bytes.length : at, false);
        payload.set(chunk, 5);
        frames.push(this.#cipher.encrypt(payload));
      }
      return frames;
    } catch { this.close(); throw new Error("The encrypted connection could not send this message."); }
  }
  receive(frame: Uint8Array): string | null {
    try {
      if (this.#closed || frame.length < 16 || frame.length > MAX_NOISE_FRAME) throw new Error();
      const payload = this.#cipher.decrypt(frame);
      if (payload.length <= 5) throw new Error();
      const position = new DataView(payload.buffer, payload.byteOffset, payload.byteLength).getUint32(1, false);
      if (payload[0] === 1) {
        if (this.#buffer || position === 0 || position > MAX_MESSAGE_BYTES) throw new Error();
        this.#buffer = new Uint8Array(position);
        this.#at = 0;
      } else if (payload[0] !== 0 || !this.#buffer || position !== this.#at) throw new Error();
      if (!this.#buffer || this.#at + payload.length - 5 > this.#buffer.length) throw new Error();
      this.#buffer.set(payload.subarray(5), this.#at);
      this.#at += payload.length - 5;
      if (this.#at !== this.#buffer.length) return null;
      const text = new TextDecoder("utf-8", { fatal: true }).decode(this.#buffer);
      this.#buffer.fill(0);
      this.#buffer = null;
      this.#at = 0;
      return text;
    } catch { this.close(); throw new Error("The encrypted connection received an invalid message."); }
  }
  close(): void {
    if (this.#closed) return;
    this.#closed = true;
    this.#buffer?.fill(0);
    this.#buffer = null;
    this.#cipher.close();
  }
}

export function lengthPrefixed(frame: Uint8Array): Uint8Array {
  if (frame.length === 0 || frame.length > MAX_NOISE_FRAME) throw new Error("Invalid frame size.");
  const bytes = new Uint8Array(frame.length + 4);
  new DataView(bytes.buffer).setUint32(0, frame.length, false);
  bytes.set(frame, 4);
  return bytes;
}

export class FrameDecoder {
  #header = new Uint8Array(4);
  #headerAt = 0;
  #body: Uint8Array | null = null;
  #bodyAt = 0;
  feed(chunk: Uint8Array): Uint8Array[] {
    const frames: Uint8Array[] = [];
    let at = 0;
    while (at < chunk.length) {
      if (!this.#body) {
        const take = Math.min(4 - this.#headerAt, chunk.length - at);
        this.#header.set(chunk.subarray(at, at + take), this.#headerAt);
        at += take; this.#headerAt += take;
        if (this.#headerAt < 4) continue;
        const length = new DataView(this.#header.buffer).getUint32(0, false);
        if (length === 0 || length > MAX_NOISE_FRAME) throw new Error("Invalid frame size.");
        this.#body = new Uint8Array(length); this.#bodyAt = 0; this.#headerAt = 0;
      }
      const take = Math.min(this.#body.length - this.#bodyAt, chunk.length - at);
      this.#body.set(chunk.subarray(at, at + take), this.#bodyAt);
      at += take; this.#bodyAt += take;
      if (this.#bodyAt === this.#body.length) { frames.push(this.#body); this.#body = null; this.#bodyAt = 0; }
    }
    return frames;
  }
}
