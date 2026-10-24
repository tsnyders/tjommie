# Android companion delivery

The owner requested both a Windows desktop application and an Android APK on 2026-10-22. The Quiet pill phone layout is shared UI, not an Android connection implementation.

## First functional APK

- Bundle the shared static UI in Capacitor for Android. Build a debug-signed APK for local testing, with explicit signing and verification limits.
- Pair by scanning or pasting a ten-minute, single-use code from the PC. Store the device identity encrypted with an Android Keystore key. Keep the PC identity in Windows Credential Manager.
- Use a separate Noise-authenticated encrypted LAN transport. Preserve the loopback-only desktop daemon and keep its bearer token off the phone and network.
- Authorize every connection against the current device list. Revocation closes existing connections and prevents reconnecting. Pairing cannot be replayed or used after expiry.
- Reuse typed RPC and events for agents, chat, approvals, routines, Accounts and Usage. Reconnect without silently repeating mutations.
- Support keyboard resizing, system Back, safe areas and external links. Browser viewing/control remains desktop-only.

The first APK requires the PC to be awake and reachable on the same network. The relay, mobile-data access and push remain separate work; do not imply they are present. No Firebase credentials or public relay are configured by this delivery.

## Implementation boundaries

Root owns protocol, device storage, daemon dispatch/listener integration, client transport abstraction, UI pairing/settings and final verification. UI Builder owns only the new mobile package/native plugins until a further explicit handoff. Runtime agent researches the maintained Noise implementation before its package contract is frozen. Dependency lock changes are serialized.

No agent may inspect owner credentials, interact with real provider accounts, stop the installed desktop app, expose plaintext daemon authentication on the LAN, or invent a cryptographic protocol. Use synthetic identities/accounts and disposable app profiles for verification.

## Verification

1. Pairing expiry, consumption, concurrent attempts, malformed frames and key mismatch.
2. Encrypted round trips, tampering/replay rejection, fresh connection keys, unknown/revoked device rejection.
3. RPC/event parity and no mutation replay after reconnect.
4. Android build plus the owner's authorized physical S25 Ultra, pairing to a disposable PC host, chat/approval UI, keyboard and Back, reconnect and revocation.
5. Desktop regression and mandatory packaged provenance gate after all final source changes.
6. Record APK signing, platform/version, artifact paths and the limits of emulator versus physical-phone acceptance.
