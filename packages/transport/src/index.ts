import { createInitiator, createResponder, generateKeys, initSync, type Session } from "../wasm/tjommie_noise.js";

export const MAX_NOISE_FRAME = 65_535;
export const MAX_NOISE_PLAINTEXT = MAX_NOISE_FRAME - 16;
export const MAX_NOISE_RECORDS = 2 ** 31;
const domain = new TextEncoder().encode("Tjommie/remote/v1");
let ready = false;
const failure = (): Error => new Error("Secure connection failed.");

export function initNoise(bytes: Uint8Array): void {
  if (ready) return;
  try { initSync({ module: bytes }); ready = true; }
  catch { throw new Error("Secure transport could not initialize."); }
}

export function generateKeypair(): { privateKey: Uint8Array; publicKey: Uint8Array } {
  if (!ready) throw failure();
  try {
    const keys = generateKeys(); if (keys.length !== 64) throw failure();
    const pair = { privateKey: keys.slice(0, 32), publicKey: keys.slice(32) }; keys.fill(0); return pair;
  } catch { throw failure(); }
}

export interface NoiseSession {
  readonly handshakeFinished: boolean;
  readonly remotePublicKey: Uint8Array | null;
  writeHandshake(payload: Uint8Array): Uint8Array;
  readHandshake(frame: Uint8Array): Uint8Array;
  encrypt(plain: Uint8Array): Uint8Array;
  decrypt(cipher: Uint8Array): Uint8Array;
  close(): void;
}
type Options = { privateKey: Uint8Array; psk?: Uint8Array; prologue?: Uint8Array };
function checked(options: Options): Uint8Array {
  if (!ready || options.privateKey.length !== 32 || options.psk !== undefined && options.psk.length !== 32 || (options.prologue?.length ?? 0) > 4096) throw failure();
  // The application context is always domain-separated, including callers that supply routing bytes.
  const prologue = new Uint8Array(domain.length + 1 + (options.prologue?.length ?? 0));
  prologue.set(domain); if (options.prologue) prologue.set(options.prologue, domain.length + 1); return prologue;
}
function wrap(native: Session): NoiseSession {
  let closed = false;
  const close = (): void => { if (closed) return; closed = true; native.close(); native.free(); };
  const run = (operation: () => Uint8Array): Uint8Array => {
    if (closed) throw failure();
    try { return operation(); } catch { close(); throw failure(); }
  };
  return {
    get handshakeFinished() { return !closed && native.handshakeFinished; },
    get remotePublicKey() { return closed ? null : native.remotePublicKey ?? null; },
    writeHandshake: (payload) => run(() => native.writeHandshake(payload)),
    readHandshake: (frame) => run(() => native.readHandshake(frame)),
    encrypt: (plain) => run(() => native.encrypt(plain)),
    decrypt: (cipher) => run(() => native.decrypt(cipher)),
    close,
  };
}
export function initiator(options: Options & { remotePublicKey: Uint8Array }): NoiseSession {
  try {
    const prologue = checked(options); if (options.remotePublicKey.length !== 32) throw failure();
    return wrap(createInitiator(options.privateKey, options.remotePublicKey, options.psk, prologue));
  } catch { throw failure(); }
}
export function responder(options: Options): NoiseSession {
  try { return wrap(createResponder(options.privateKey, options.psk, checked(options))); }
  catch { throw failure(); }
}
