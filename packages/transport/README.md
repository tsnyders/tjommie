# Tjommie secure transport primitive

This package wraps **Snow 0.10.0**, unchanged, compiled to WebAssembly. Snow implements
the Noise handshake, X25519, ChaCha20-Poly1305, BLAKE2s and transport nonce state.
Our Rust code exposes its state machine and bounds; it does not implement cryptography.
Snow's upstream README states that the library has not received a formal audit.

Both Node/Electron and the Android WebView use the same checked-in local WASM asset.
Import the TypeScript API from `@tjommie/transport`, load the local bytes using the
platform's asset loader, then call `initNoise(bytes)`. No runtime download occurs.
The explicit byte initializer also avoids browser/Node filesystem assumptions.

## API

- `initNoise(bytes: Uint8Array): void`
- `generateKeypair(): { privateKey: Uint8Array; publicKey: Uint8Array }`
- `initiator({ privateKey, remotePublicKey, psk?, prologue? }): NoiseSession`
- `responder({ privateKey, psk?, prologue? }): NoiseSession`
- A session provides `writeHandshake`, `readHandshake`, `handshakeFinished`,
  `remotePublicKey`, `encrypt`, `decrypt`, and idempotent `close`.

Keys and PSKs must be exactly 32 bytes. A PSK selects
`Noise_IKpsk2_25519_ChaChaPoly_BLAKE2s`; its absence selects
`Noise_IK_25519_ChaChaPoly_BLAKE2s`. The fixed application domain is always included
in the Noise prologue. Pass identical canonical routing bytes as `prologue` on both
peers to bind version, host identity, pairing mode, and public pairing identifier.

## Application responsibilities

**Handshake completion is not application authorization.** In IKpsk2, the first
message does not prove knowledge of the PSK. The host must authenticate an encrypted
post-handshake client confirmation before consuming the invitation or persisting the
device key. For later IK connections it must check the recovered device public key
against the current non-revoked device list. Neither mode permits early RPC payloads.

The coordinator owns the ten-minute one-use invitation, authorization, revocation,
TCP framing, application fragmentation, and connection deadlines. The daemon token
is never sent to a remote peer. The one-use PSK stays in host memory until expiry/use;
hash-only persistent records cannot initialize a PSK handshake.

Noise ciphertext frames are at most 65,535 bytes; transport plaintext is at most
65,519 bytes. A session closes on authentication, state, or bounds failure. Both
directions stop before 2^31 transport records and must establish a fresh connection.
Do not retry a failed frame or reset a nonce. Destroy both session objects on exit.

Long-lived host keys belong in Windows Credential Manager. The Android lane wraps
the device private key with an Android Keystore AES-GCM key. The wrapping key is
non-exportable; the Noise key enters JS/WASM memory while in use. This is encrypted
at-rest storage, not a claim that X25519 stays inside secure hardware. Callers should
clear their temporary byte arrays. `close()` drops the opaque Snow state and forbids
reuse; it does not promise complete zeroization of JavaScript/WASM allocator memory.

## Rebuilding

Rust 1.85 or later and `rustup target add wasm32-unknown-unknown` are required.
Run `node packages/transport/scripts/build.ts` from the checkout. The script uses
the committed Cargo lockfile and wasm-bindgen **0.2.129**. On Windows x64 it downloads
the upstream build tool into the ignored `.tools` directory and checks the pinned
SHA-256 before extraction. Other platforms provide `TJOMMIE_WASM_BINDGEN` pointing
to the same CLI version. Generated JS and WASM are local release assets; the narrow
declarations are maintained alongside the wrapper.

Sources: [Snow](https://github.com/mcginty/snow/tree/v0.10.0),
[Noise specification](https://noiseprotocol.org/noise.html),
[wasm-bindgen](https://github.com/wasm-bindgen/wasm-bindgen/releases/tag/0.2.129),
[Android Keystore](https://developer.android.com/privacy-and-security/keystore).
Snow is Apache-2.0 OR MIT; wasm-bindgen is Apache-2.0 OR MIT. Dependency licenses
remain governed by their upstream packages and the committed Cargo lockfile.
