// Narrow declarations for the pinned local wasm-bindgen output. No private keys in diagnostics.
export class Session {
  readonly handshakeFinished: boolean;
  readonly remotePublicKey: Uint8Array | undefined;
  writeHandshake(payload: Uint8Array): Uint8Array;
  readHandshake(frame: Uint8Array): Uint8Array;
  encrypt(plain: Uint8Array): Uint8Array;
  decrypt(cipher: Uint8Array): Uint8Array;
  close(): void;
  free(): void;
}
export function generateKeys(): Uint8Array;
export function createInitiator(privateKey: Uint8Array, remoteKey: Uint8Array, psk: Uint8Array | undefined, prologue: Uint8Array): Session;
export function createResponder(privateKey: Uint8Array, psk: Uint8Array | undefined, prologue: Uint8Array): Session;
export function initSync(input: { module: Uint8Array }): unknown;
