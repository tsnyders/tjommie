# Tjommie for Android (`packages/mobile`)

An installable Android companion around the shared UI, with an authenticated, encrypted connection to the PC over the LAN or its configured relay. Pair from Settings → Devices on the PC, then scan or paste the ten-minute code on the phone. The host must remain awake with Tjommie running.

## What is here
- `native/java`: the plugins, copied into the generated Android project by `scripts/apply-native.mjs`.
  - `TjommieSecureStore`: secrets sealed with an AES-256-GCM key in the Android Keystore, in private preferences left out of backups.
  - `TjommieSocket`: raw TCP carrying length-prefixed Noise frames of at most 65,535 bytes, with limits on connections and write backlogs.
  - `TjommieSocket` also reports Wi-Fi, cellular, other, and lost-network changes so the UI reconnects promptly.
  - `TjommiePairingScanner`: Google's code scanner (no camera permission of ours); pasting the code is the fallback.
  - `TjommiePushRouting`: the high-importance `bots` channel and whether this build has Firebase; Capacitor Push Notifications routes taps to JS.
- `src`: typed wrappers (`createPlugins`, base64 helpers, `codeFromScan`) with tests. No pairing or connection logic.
- `native/java-test`: JVM unit tests for the frame codec and the host checks.

## Build a test APK
1. From the repo root, after `npm install`: `npm run build` (the UI and local WASM asset).
2. `npm run android:add -w @tjommie/mobile` once, then `npm run android:sync -w @tjommie/mobile` after UI changes.
3. `npm run android:debug -w @tjommie/mobile`. The debug-signed APK is `android/app/build/outputs/apk/debug/app-debug.apk`; install it with `adb install -r`.

Debug signing is for local testing only. A release build needs a signing key that you create and keep outside the repo; none is made here.

`npm run package:android` builds and syncs the current UI, checks every exported UI asset against its bytes inside the debug APK, and records source and artifact hashes under `dist-mobile/`.

For a release APK, set `TJOMMIE_ANDROID_KEYSTORE`, `TJOMMIE_ANDROID_KEYSTORE_PASSWORD`, `TJOMMIE_ANDROID_KEY_ALIAS`, and `TJOMMIE_ANDROID_KEY_PASSWORD`, then run `npm run package:android -- --release` from the repo root. The output is `dist-mobile/Tjommie-Android-<version>.apk` and its matching provenance JSON. Packaging refuses missing signing values or a missing keystore.

To enable push, keep Firebase's `google-services.json` outside the repo and set `TJOMMIE_GOOGLE_SERVICES_JSON` to its path before packaging. `native:apply` copies it into the ignored Android app directory. Without it, the APK reports push unavailable and never registers with Firebase.

The transport uses the same pinned Rust Snow WASM module on the PC and phone: IKpsk2 for first pairing and IK for reconnecting. Snow is maintained but explicitly unaudited upstream. The Android Keystore key wraps the phone identity and its pinned PC key; the Noise private key enters JS/WASM memory while connecting. Pairing candidates remain separate from working identities until confirmation, including recovery after a lost acknowledgement.

## Limits
The PC must be awake and connected to the relay for use away from the LAN. There is no background service, so the socket drops in deep Doze and reconnects on return or a network change; push covers notifications while the app is away. Push needs Firebase credentials for this APK and the matching relay.
File attachment picking and native browser viewing/control remain desktop-only. Each paired phone has full owner authority, including account changes and pairing/revoking devices; revoke a lost phone from Settings → Devices. An unauthenticated LAN attacker may interfere with availability, although pending connection pools and frame/message sizes are bounded.
