# P0 voice: local runtime and composer feasibility

Date: 2026-11-03. Baseline: `aaaf09fe6c45cce2fe42bf9488ac3815156f4ad9`, branch `phase4-workspace-20261103`.

## Decision and gate

The Windows candidate is a CPU-only, CI-built `whisper-cli` from stable whisper.cpp **v1.9.5**, with an explicitly downloaded English `base.en` model. The Android candidate is the **explicit on-device** `SpeechRecognizer`, with installed-language checks. Speech stays on the capturing device and does not use the daemon, relay, provider account or another PC. Transcript insertion is a separate draft edit; recognition never invokes Send or a tool.

This is a feasibility decision, **not measured dictation readiness or P6 acceptance**. No speech engine/model was downloaded or executed, no microphone was accessed, and no owner phone/app/data was touched. The accepted desktop-and-Android goal remains open until offline device acceptance passes. API limitations below are proposed unsupported configurations, not an implicit approval to narrow Android compatibility.

P0 changed only this record and `scripts/phase4-spikes/voice/**`. Production changes, manifest/bridge wiring, CI build changes and model downloads wait for coordinator release. No package/lock edits, installation, release packaging, commits or push occurred in this lane.

## Existing seams and user risks

| Existing seam | Proposed reach and risk |
| --- | --- |
| `packages/ui/components/chat/composer.tsx` | Local text/attachments and revision-safe sends already exist. Add a field ref and the shared dictation action without replacing attachments or invoking `submit`. |
| `packages/ui/components/room/room-composer.tsx` | Reuse its field, caret, mentions, revision and send guard. Insertion must preserve mention behavior and typed edits. |
| `packages/ui/components/exchange/exchange-view.tsx` | `ExchangeComposer` currently receives bots, not the exchange identity. Pass a stable exchange ID for result fencing; preserve its explicit Everyone posting behavior. |
| `packages/ui/lib/use-draft.ts` | Reuse `useRevision`, `useAlive`, `useSendGuard`; they do not alone identify host changes or a new recording in the same mounted composer. |
| `packages/desktop/src/main.ts:167` | Default-session permissions currently deny everything. A narrow trusted-window audio grant is required. Broadly allowing `media` would expose camera/microphone access to other requesters. |
| `packages/desktop/src/native-browser-permissions.ts` | Bot browser profiles remain deny-all. A website must not gain microphone access through dictation. |
| `packages/desktop/src/daemon-process.ts` | Existing child supervision uses argument arrays, `windowsHide` and tree termination. Follow that pattern in a speech-specific supervisor without changing daemon behavior. |
| `packages/mobile/native/` and `scripts/apply-native.mjs` | Native source is authoritative; generated `android/` copies alone are insufficient. Existing Android minimum is API 24; a native speech API floor is a feature gate, not an app-wide SDK bump. |

No wire/protocol, database, provider or paid inference change is needed for local dictation. Host switching still invalidates a draft's pending speech result, even though recognition itself is local. The app must not condition microphone/model availability on `useLocalFileActions()` or a live host connection.

## Runtime and model provenance

The machine-readable record is [provenance.json](../../../scripts/phase4-spikes/voice/provenance.json). Public metadata was queried directly; **metadata identity checks are not downloaded-byte verification**.

| Item | Verified upstream candidate |
| --- | --- |
| Runtime | [ggml-org/whisper.cpp v1.9.5](https://github.com/ggml-org/whisper.cpp/releases/tag/v1.9.5), published 2026-10-06. Stable release metadata contained **zero binary assets** when checked. |
| Immutable source | Commit `d1be6fde11ac6e0407606b4e42fe72d34add8037`; annotated tag object `d2a764c648779feb36d44363f212a29b00aad1af`. GitHub's [commit metadata](https://api.github.com/repos/ggml-org/whisper.cpp/commits/d1be6fde11ac6e0407606b4e42fe72d34add8037) reports a verified commit; the separate annotated tag object is unsigned. Pin the commit, not just a mutable tag/latest URL. |
| Selected model | `ggerganov/whisper.cpp`, revision `5359861c739e955e79d9a303bcbc70fb988958b1`, `ggml-base.en.bin`, **147,964,211 bytes** (141.11 MiB). [Pinned metadata](https://huggingface.co/api/models/ggerganov/whisper.cpp/tree/5359861c739e955e79d9a303bcbc70fb988958b1?recursive=false&expand=false). |
| Model SHA256 | `a03779c86df3323075f5e796cb2ce5029f00ec8869eee3fdfb897afe36c6d002`, from the pinned repository's LFS object identity. Stream and hash the actual payload before making it usable. |
| Runtime size/hash | **Unknown until CI builds it.** No invented binary digest or install-size estimate. |
| Licenses | Runtime/ggml notices are MIT: [pinned LICENSE](https://github.com/ggml-org/whisper.cpp/blob/d1be6fde11ac6e0407606b4e42fe72d34add8037/LICENSE). The converted model repository declares MIT; OpenAI states its [Whisper code and model weights use MIT](https://github.com/openai/whisper#license). Preserve both upstream notices and conversion provenance. |

### Windows packaging choice

Build the pinned source in GitHub Actions; do not fetch a floating nightly executable. The initial target is the app's existing Windows x64 architecture, CPU only. Place the CLI and required notices in `resources/speech/` outside `app.asar`. Build the CLI target only, without server, SDL, libcurl, ffmpeg or GPU backends. Native Web Audio produces PCM, so a multimedia conversion dependency is unnecessary.

Proposed CMake inputs, checked against the [pinned top-level](https://github.com/ggml-org/whisper.cpp/blob/d1be6fde11ac6e0407606b4e42fe72d34add8037/CMakeLists.txt) and [ggml options](https://github.com/ggml-org/whisper.cpp/blob/d1be6fde11ac6e0407606b4e42fe72d34add8037/ggml/CMakeLists.txt):

```text
-A x64
-DBUILD_SHARED_LIBS=OFF -DGGML_BACKEND_DL=OFF
-DWHISPER_BUILD_TESTS=OFF -DWHISPER_BUILD_EXAMPLES=ON
-DWHISPER_CURL=OFF -DWHISPER_COMMON_FFMPEG=OFF
-DGGML_CUDA=OFF -DGGML_VULKAN=OFF -DGGML_OPENMP=OFF
-DGGML_NATIVE=OFF -DGGML_SSE42=OFF -DGGML_AVX=OFF -DGGML_AVX2=OFF
-DGGML_FMA=OFF -DGGML_F16C=OFF
-DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded
cmake --build build --config Release --target whisper-cli
```

These are proposed build inputs, not a successfully compiled artifact. Disabling host-specific instruction sets avoids accidentally requiring the CI CPU; throughput may suffer. P6 must verify the static/CRT result, transitive libraries, notices and minimum CPU behavior, then record executable bytes/SHA256, source/tree identity, compiler/SDK versions, flags and CI run. Check runtime hashes at packaged verification and before execution. Unsupported architectures report Unavailable rather than executing another binary. No code signing claim is made for this upstream-derived helper.

Use one supervised subprocess, argument arrays and no shell. Suggested starting limits are one recording/transcription, at most four inference threads, 60 seconds of mono 16 kHz PCM16 (1,920,000 payload bytes; 1,920,044 with WAV header), bounded 64 KiB bridge chunks, a 120-second transcription timeout, a 128 KiB JSON result ceiling and 8,192 transcript characters. These are resource policies to test, not performance measurements or a hard process-memory cap. A base model's memory behavior still needs measurement before shipping.

The [CLI source](https://github.com/ggml-org/whisper.cpp/blob/d1be6fde11ac6e0407606b4e42fe72d34add8037/examples/cli/cli.cpp) supports `--no-gpu`, `--threads`, `--processors`, `--language en`, `--output-json`, `--output-file`, `--no-prints`, and the no-speech threshold. Write input/output only to an app-owned unique session directory; discard stdout/stderr instead of logging transcripts or upstream output. Validate bounded JSON before emitting a final event. On cancel/timeout/quit kill the process tree, await exit, delete that directory, and retain a cleanup failure state instead of reporting successful cleanup. On next start remove only stale session directories owned by this feature. No arbitrary renderer-supplied paths or CLI flags.

### Model lifecycle

Model states: Missing → Downloading → Verifying → Ready, with explicit Cancelled/Error and a separate Removing state. The user initiates a clearly sized download; recording does not start a download or quietly use cloud speech. Use the fixed, revision-pinned HTTPS URL and a bounded redirect policy for verified upstream CDN hosts; accept neither renderer URLs nor arbitrary local models in this version.

Stream into an exclusive `.partial` file in current-device app storage, enforce byte count while reading, verify exact length and SHA256, then atomically publish a digest-named model. A truncated, oversized or hash-mismatched result never becomes Ready. Cancel aborts the request, closes/deletes the partial and fences late completion. First version restarts a cancelled download instead of adding resume behavior. Check disk space and report ENOSPC without touching an existing verified model. Revalidate an existing model before use; deletion cancels/waits for active speech before removing only the managed model. No uninstall/account/chat reset is part of this operation.

No raw audio belongs in persistent settings/diagnostics; only engine/version, selected language, model byte/hash verification status, permission/error code and numeric timing/size are eligible. Temporary-file deletion is ordinary deletion, not a secure-erasure guarantee for OS paging/crash behavior.

## Capture and permissions

Windows uses `getUserMedia({audio: ..., video: false})` from the trusted app main frame after an explicit microphone action. A standard [AudioWorklet](https://www.w3.org/TR/webaudio-1.0/#AudioWorklet) captures mono samples; request a 16 kHz context and validate its actual sample rate. If the device/context cannot provide that path, show Unsupported until a tested resampler exists. Do not label compressed MediaRecorder/WebM data as WAV or add ffmpeg merely to convert it.

Root-owned Electron handlers must check the exact app `WebContents`, main frame, allowed origin (`app://tjommie` in production, exact localhost origin only in development), an active short-lived user-started capture lease, and audio-only media type. Implement both [request and check handlers](https://www.electronjs.org/docs/latest/api/session#sessetpermissionrequesthandlerhandler); reject missing/ambiguous media details, video, display capture, subframes, guest profiles and unrelated permissions. Keep bot-browser handlers unchanged. Revoke the lease on lifecycle cancellation. Windows privacy settings or an absent input device can still deny capture; surface a sanitized error and stop all acquired tracks.

`getUserMedia` can finish after Cancel. Fence its resolution **before** building the graph; immediately stop every returned track if the session is stale. Stop tracks, disconnect worklets, close the context and clear buffers on completion/cancel/error, navigation, host change, unmount, hidden/minimized/backgrounded app or native activity pause. A late promise/event must never restart capture. A pending permission prompt itself cannot be reliably dismissed by browser code; cancellation guarantees any later stream is discarded.

Android uses a native Capacitor speech plugin, not WebView `getUserMedia`; it requires no broad WebView microphone grant. Add/request `RECORD_AUDIO` only on Start and query the recognition service through the source-driven manifest patch. Native start checks trusted app origin, foreground activity, current session token and current permission after any permission callback. No foreground microphone service, background recording or wake lock is proposed.

## Android API and fallback boundary

The [SpeechRecognizer API](https://developer.android.com/reference/android/speech/SpeechRecognizer) supplies `isOnDeviceRecognitionAvailable` and `createOnDeviceSpeechRecognizer` from API 31, plus support queries from API 33. Use the main thread, install the listener before commands, wait for final/error after Stop, and cancel/destroy on cancellation or activity teardown. Never substitute `createSpeechRecognizer`. [`EXTRA_PREFER_OFFLINE`](https://developer.android.com/reference/android/speech/RecognizerIntent#EXTRA_PREFER_OFFLINE) may be ignored by an implementation and is not the privacy boundary.

| Configuration | Proposed result, pending device validation |
| --- | --- |
| API 33+, explicit on-device service, selected tested English locale in `getInstalledOnDeviceLanguages()` | Enable local dictation. Query support using the same intent/language that will start recognition; do not equate available-for-download with installed. |
| API 33+, missing language/pending download | Download needed; direct to system speech/language management, then recheck. |
| API 31–32 | An on-device factory exists, but the installed-language support query does not. First implementation reports Unavailable; supporting this range needs a separately validated path. |
| API 24–30 or no explicit on-device service | Unavailable with manual typing retained; no cloud fallback. |

The [support lists](https://developer.android.com/reference/android/speech/RecognitionSupport) distinguish installed, supported-but-not-installed, pending and online languages. Start with tested English locales only; the Windows `.en` model exposes English, not a multilingual promise. Android's selectable list is the intersection of installed locales and fixture-validated locales. English default and each advertised locale still require offline speech fixtures. No South African-language, accent, punctuation or identifier accuracy claim is supported by this spike.

The platform exposes model-download initiation but no app-level cancellation/removal method. Therefore this first path **does not initiate system model downloads** or claim the app can cancel/delete them. Label system-managed language data explicitly. The service controls its own processing/storage; Tjommie can avoid retaining raw audio and request on-device processing, but must not claim it deletes the recognizer's private storage. Silence/noise handling, callbacks after cancellation and the maximum-duration timer require device proof.

An Android whisper.cpp fallback is technically plausible: upstream includes an [Android example](https://github.com/ggml-org/whisper.cpp/tree/d1be6fde11ac6e0407606b4e42fe72d34add8037/examples/whisper.android) with JNI/CMake integration and recommends tiny/base. It is **not selected** for this wave: NDK/ABI packaging, JNI cancellation/abort callback, AudioRecord capture, lifecycle, native crash recovery, memory/thermal/battery behavior and model ownership would all need implementation and measurement. The pinned English tiny alternative is 77,704,715 bytes with SHA256 `921e4cf8686fdd993dcd081a5da5b6c365bfde1162e72b08d75ac75289920b1f` (metadata in provenance). This is an unresolved route for broader Android compatibility, not proven fallback behavior. No remote-host transcription workaround is acceptable when the PC is offline.

## Draft and lifecycle contract

Use one small common hook/action and pure draft insertion helper for chat, room and exchange. Capture `{hostIdentity, conversationKind, conversationId, mountToken, generation, requestId, selectionStart, selectionEnd}` before permission/capture. Use immutable host identity rather than a display name; every new start has a new ID. A detached composer or changed host invalidates its token immediately. Do not use daemon connectivity itself to cancel purely local transcription unless the conversation/host identity actually changes.

States are Idle, Permission, Listening, Transcribing, Conflict, Error and Unavailable. Tap Start/Stop, with separate Cancel. Stop ends audio and requests final text; Cancel discards it. Use `type="button"`, an explicit microphone accessible name, listening/stop names and a polite status region. Keep Send an explicit existing action, support keyboard focus and reduced motion, and test 360/390 px layouts/text scaling. Partial recognizer text is status-only and never mutates the draft.

For a final result, require matching IDs/scope and a still-transcribing session, then consume it once. Rebase the captured range across observed edits entirely before/after it. A typed insertion exactly at an empty anchor is kept before the eventual transcript. If an edit overlaps the selected range, or an IME/undo/autofill edit cannot be mapped reliably, keep every current character and show the final transcript in that originating composer's Conflict state with explicit Insert/Discard. Insert uses the person's current selection; it is never a deferred automatic replacement. Native results must not carry raw draft content.

The synthetic test accepts exact edit ranges; it **does not prove browser beforeinput/IME mapping**. Production must capture and validate actual edit ranges, or invalidate the range on an ambiguous edit. Existing selected text is replaced only while the anchor remains valid. Attachments, incoming messages and caret-only changes do not clear drafts. Starting another recording, Send, navigation or backgrounding cancels/fences the old session. Send may send the text currently shown; no pending final result may repopulate the cleared draft afterwards. Existing send revisions/guards must still protect text typed during an in-flight send.

## Silence/noise decision

Start with built-in decoder filtering and an exact-zero input rejection; **do not add VAD automatically**. The [pinned decoder](https://github.com/ggml-org/whisper.cpp/blob/d1be6fde11ac6e0407606b4e42fe72d34add8037/src/whisper.cpp) defaults to no-speech threshold `0.6` and log-probability threshold `-1.0`; its no-speech decision combines both. These heuristics are not a guarantee against hallucinated noise/silence. Do not interpret the unit test's synthetic `speechDetected` flag as measured recognizer evidence or use RMS alone to classify speech.

P6 must run silent, low-level noise, fan/keyboard noise, music and speech fixtures on the chosen runtime/model before accepting successful dictation. A nonempty transcript alone is insufficient. If those checks fail, evaluate the existing upstream VAD option against the same fixtures, measure false rejection of quiet speech and added latency, then select it only with evidence. The unselected Silero VAD 6.2.0 candidate is 885,098 bytes; pinned identity is recorded for review, not bundled/downloaded. Neither a VAD model nor confidence thresholds establish an absolute hallucination guarantee.

## Checks actually performed

| Check | Observed result and limit |
| --- | --- |
| `node --test scripts/phase4-spikes/voice/contracts.check.ts` | **11/11 passed**, 0 skipped, 189.1287 ms reported duration on the latest run. Renamed from `contracts.test.ts` during P6 so root Vitest does not collect this Node-only check. Pure synthetic model: insertion, edits/conflict, attachments, host/kind/target/mount/generation fences, cancelled/pending/stale/duplicate finals, no-speech outcome, audio ceiling, byte/hash mismatch and provenance shape. No actual composer/native/recognizer behavior is proved. |
| `node scripts/phase4-spikes/voice/capture-feasibility.cjs` | Passed in a hidden isolated Electron process/profile; cleanup completed. Electron **44.4.5**, Chromium **152.0.7977.130**, Windows x64. `app://tjommie/` was a secure context; microphone API and AudioWorklet existed. An offline AudioWorklet rendered 16,000 generated zero frames at 16,000 Hz. All permission handlers denied; no `getUserMedia` invocation, microphone, speaker or real recording. This proves the synthetic worklet/secure-origin seam, not hardware input/resampling or permission UX. |
| Upstream metadata | Stable tag dereferenced to the pinned commit; commit verification and empty release asset list inspected. Pinned HF revisions returned exact model sizes/LFS SHA256 values. Only text/JSON metadata/source was fetched. No runtime/model byte verification or transcription benchmark occurred. |
| Static checks | `node scripts/check-no-any.ts` passed across 466 TypeScript files in P0. The spike TypeScript passed a focused `tsc --noEmit --module nodenext --moduleResolution nodenext --target es2023 --strict --noUncheckedIndexedAccess --skipLibCheck --types node scripts/phase4-spikes/voice/contracts.check.ts`. Repository `git diff --check` passed. Full app typecheck/lint/tests were not run for the isolated P0 spike. |

The speech engine's latency, peak working set, CPU use, model load/cancel time, packaged disk increase and Android battery/thermal behavior are **unmeasured**. The 60-second PCM count is a mathematical resource ceiling, not recording duration/quality evidence. No physical-device, permission-dialog, accessibility, production app, package or real-provider test occurred.

## Proposed P6 implementation allowlist

These are requested future lanes, not permission to edit them now. Coordinator should reconcile them with other feature owners before release.

**VOICE-owned additions/edits:**

- `packages/ui/lib/dictation.ts`, `dictation.test.ts`, `use-dictation.ts`, `use-dictation.test.tsx`, `local-speech.ts`, `local-speech.test.ts`.
- `packages/ui/components/chat/dictation-button.tsx`, `dictation-button.test.tsx`, `composer.tsx`, `composer.test.tsx`.
- `packages/ui/components/room/room-composer.tsx`, `room-composer.test.tsx`.
- `packages/ui/components/exchange/exchange-view.tsx`, `exchange-composer.test.tsx` (including the existing ExchangeComposer caller's identity prop).
- `packages/ui/public/audio/pcm-capture.worklet.js`.
- `packages/desktop/src/speech-runtime.ts`, `speech-runtime.test.ts`, `speech-model.ts`, `speech-model.test.ts`, `speech-permissions.ts`, `speech-permissions.test.ts`.
- `packages/mobile/src/speech.ts`, `speech.test.ts`.
- `packages/mobile/native/java/dev/tjommie/app/TjommieSpeechPlugin.java`, `SpeechSession.java`; matching `packages/mobile/native/java-test/dev/tjommie/app/SpeechSessionTest.java` and `TjommieSpeechPluginTest.java`.
- This decision record and `scripts/phase4-spikes/voice/**`, including synthetic fixtures/results. No model binaries/audio recordings in Git.

**Coordinator/shared-file integration requests:**

- `packages/desktop/src/main.ts`, `preload.cjs`: narrow local IPC/event surface, trusted sender/frame validation, permission lease and quit/minimize cleanup. Proposed methods: `status`, `downloadModel`, `cancelModelDownload`, `removeModel`, `start`, `appendPcm`, `stop`, `cancel`, `onState`, `onFinal`; no arbitrary path/model URL/process parameters. Each session method validates ID, state, chunk bytes, total bytes and sender.
- `packages/ui/lib/desktop.ts`: optional typed local-speech bridge; microphone/input/language/model settings remain current-device scoped. No provider/daemon/protocol API needed.
- `packages/mobile/native/java/dev/tjommie/app/MainActivity.java`: source plugin registration; source-driven `packages/mobile/scripts/apply-native.mjs` plus `apply-native.test.ts`: microphone permission and recognition-service query. Generated copies come only from normal native sync. Shared mobile exports may need `packages/mobile/src/index.ts` integration.
- `packages/ui/app/settings/page.tsx` and its future split settings components: Voice section, installed local model/permission/engine status and system-managed Android wording. Root/settings owner integrates it after agreeing the status contract.
- `.github/workflows/ci.yml`, `.github/workflows/release.yml`, `packages/desktop/scripts/package.ts`, `build-provenance.ts` and packaged smoke fixtures: source-pinned runtime build, licenses/resource inclusion, runtime digest/size and offline synthetic speech fixture checks. No workflow was changed or run here.
- `CHANGELOG.md`: plain-English dictation entry with the actual supported configurations when a user-visible implementation lands. No P0-only changelog claim. Root owns lockfiles/manifests; no new npm dependency is currently proposed.

**Required before accepting P6:** CI compile/package/hash/license proof; actual Windows capture and resampling/permission denial; owned model cancel/error/removal/restart/ENOSPC cases; end-to-end draft/Send/IME/navigation/background/late-permission races across all three composers; bounded process/audio/output behavior and raw-data cleanup; offline Android installed-locale tests and activity teardown; silence/noise/speech quality fixtures; measured Windows/Android resource behavior and accessibility/layout checks. Broader Android/API31–32/JNI fallback support remains unresolved until tested. P0 does not reopen Phase 3 or authorize owner-device access to close these gates.

## P6 source implementation handoff

The coordinator released the VOICE lane for source implementation after reviewing P0. The code below implements the local path; it **does not establish P6 native/hardware acceptance**. Shared main/preload/manifest/CI/settings/changelog files remain coordinator-owned. No runtime/model was downloaded, no microphone/phone/owner profile was accessed, and no local app/release build, commit or push was performed.

### Implemented source

- `packages/protocol/src/speech.ts`: validated current-device `SpeechApi`, `SpeechStatus`, `SpeechEvent`, start/session/PCM schemas and resource limits. No daemon/wire method or migration. The coordinator owns its index export.
- `packages/desktop/src/speech-model.ts`: fixed revision/hash model identity, regular-file validation, streaming HTTPS download with an upstream-host redirect allowlist, free-space preflight, length/size/SHA256 checks, exclusive partials, atomic publication, cancellation and removal. Model-download progress is bounded to percentage changes. Public model data is not a transcription provider.
- `packages/desktop/src/speech-runtime.ts`: hash-verified regular runtime file, one session, English-only CLI invocation, ordered/bounded PCM, WAV header, CPU/thread limits, capture/inference deadlines, sanitized bounded JSON parsing, tree cancellation, and managed-directory cleanup before emitting final text. No binary/hash means honest Unavailable. Startup removes only owned stale session directories. Child stdout/stderr are discarded. Digital silence is rejected before inference; heuristic noise quality remains an acceptance gate.
- `packages/desktop/src/speech-permissions.ts`: exact trusted app contents/origin/main-frame/audio-only lease checks for both Electron handlers. Browser guest permissions are not changed by this helper.
- `packages/mobile/src/speech.ts`: schema-checked native status/events, listener teardown, pending-start cancellation fence, and explicit refusal of app-managed system-model or PCM operations. No online recognizer fallback.
- `packages/mobile/native/java/dev/tjommie/app/{TjommieSpeechPlugin,SpeechSession}.java`: foreground/trusted-origin checks, runtime microphone permission, API 33+ explicit on-device service/support query, installed English language verification, main-thread recognizer commands, bounded duration/final text, cancellation/generation fences, and pause/navigation/destroy teardown. System-owned model downloads are never initiated. Cleanup failures suppress successful final text and make the service unavailable until released/restarted.
- `packages/ui/lib/{dictation,local-speech,use-dictation}.ts` and `public/audio/pcm-capture.worklet.js`: one capture owner, late-stream cleanup, 16 kHz mono PCM16 worklet, four-chunk backlog, bounded duration/bytes, muted output, scoped session/result fences and draft insertion. Native `beforeinput` ranges handle edits before/after the anchor; overlapping, identical selected replacements, IME/history or unknown edits become an explicit Insert/Discard conflict. No transcript is submitted automatically. Conflict text cannot display/insert after a host/conversation change.
- Chat, room and exchange composers all use the same hook/control; Send cancels/fences pending speech before existing send logic. Attachments, mentions and revision-safe send clearing remain intact. ExchangeComposer now receives its actual exchange ID. New speech controls have explicit labels, status announcements and 44 px target dimensions.

The local language and microphone choices use `tjommie.voice.language` and `tjommie.voice.inputDevice` in current-device local storage. Device names are enumerated only after permission/capture succeeds. No transcript/audio is stored there. No connection/daemon check gates the local engine; the existing AppShell retains cached screens while reconnecting/offline, and a synthetic hook test exercises offline-host draft insertion. A cold start without cached conversation data is not an added offline-history feature.

### Coordinator integration contract

`DesktopApi.speech?: SpeechApi` is added in `packages/ui/lib/desktop.ts`; that file is released back to the coordinator for other bridge typing. Instantiate exactly one `SpeechRuntime` for the current device:

```ts
new SpeechRuntime({
  runtimePath: join(process.resourcesPath, "speech", "whisper-cli.exe"),
  runtimeSha256: trustedPackagedManifestDigestOrNull,
  dataDir: join(app.getPath("userData"), "speech"),
  emit: (event) => { /* send speech:event only to the live trusted app window */ },
});
```

`trustedPackagedManifestDigestOrNull` means a digest bundled by the reviewed CI artifact manifest, never a renderer/env-supplied substitute. The example's delivery comment is an integration instruction; the engine implementation emits actual validated results, not a successful placeholder.

Expose `status`, `downloadModel`, `cancelModelDownload`, `removeModel`, `start`, `appendPcm`, `stop`, `cancel` through exact trusted sender/main-frame-checked IPC. `onEvent` is the preload's listener for `speech:event`. Every session argument is schema-validated inside the service too. Root must reject untrusted senders **before** calling the service. Root owns channel names/preload wiring; no speech method belongs on the remote daemon bridge.

Call `installSpeechPermissions(session.defaultSession, trustedAppContents, () => speech.isCapturing(), production)` in place of the app default deny handlers; leave native bot-browser sessions deny-all. `isCapturing()` is the permission lease. `isRecordingAudio()` additionally requires received PCM and can gate blur handling so a system permission dialog does not cancel its own initial request. The UI ignores blur during Permission, rejects late PCM acquisition unless visible/focused, and always cancels on hidden/pagehide/unmount/scope change; native Android pause independently cancels. Root must cancel/await `cancelAll()` on minimize/renderer navigation or loss, shutdown and other trusted teardown. The Windows supervisor currently follows the existing managed tree-kill pattern; crash-bound process lifetime would require a Windows Job Object and is not claimed.

Android registration remains root-owned: add `registerPlugin(TjommieSpeechPlugin.class)` in native-source MainActivity; add `RECORD_AUDIO` and the `android.speech.RecognitionService` query through `apply-native.mjs` plus its source tests. Run normal native sync to copy source and tests. Do not edit generated Android copies alone. The full plugin must compile/run its JVM fixture after that integration; the pure session tests do not establish that compile gate.

Root also retains the CI runtime build/asset hashes/licenses, packaged provenance/smoke checks, Voice settings/health diagnostics and the plain-English changelog entry. Speech diagnostics may use sanitized status fields only; exclude `final.text`, PCM, temp paths and upstream stdout/stderr. The UI currently provides local language/model/microphone controls within the opened dictation controls so the workflow remains usable before settings organization lands.

### Source checks and remaining gates

- Focused Vitest selection: `packages/desktop/src/speech-model.test.ts`, `speech-runtime.test.ts`, `speech-permissions.test.ts`; `packages/mobile/src/speech.test.ts`; `packages/ui/lib/dictation.test.ts`, `local-speech.test.ts`, `use-dictation.test.tsx`; chat dictation/composer, room composer and exchange composer tests. **64/64 passed across 11 files**, 9.27 seconds, on the final run. This includes navigation/edits while awaiting an earlier cancellation, restart partial cleanup, refusal to start while a child is stopping, and failure to delete temporary audio suppressing final text. Fixtures use only generated text/PCM, tiny synthetic model/runtime files, a disposable Node child, and mocked bridge/provider results. No real recognition success is inferred from them.
- `node scripts/phase4-spikes/voice/jvm-check.mjs`: **5/5 passed**, JUnit reported 0.012 seconds. Compiles authoritative `SpeechSession.java` and its source test to an asserted disposable temp directory using cached JUnit/Hamcrest, then deletes it. No Android/app build. `TjommieSpeechPluginTest.java` exists for the coordinator's integrated Android compile/test gate and was not represented as run here.
- Focused desktop/protocol TypeScript passed with strict, erasable-syntax-only checks; mobile workspace TypeScript and scoped UI ESLint passed. `next typegen` generated route types successfully without an app build. The last full UI TypeScript invocation returned **46 errors across six projects/attachments integration files, zero voice-file diagnostics**; this is not a green full UI gate. The coordinator has the affected file list. The latest no-any scan passed across **526** TypeScript files. The original P0 Node check still passes **11/11** after its `.check.ts` rename. Voice-owned whitespace/newline checks and scoped tracked diff whitespace checks passed.

Open accepted-scope gates: root bridge/manifest/native-source integration; integrated plugin compilation; trusted CI binary/model payload/provenance checks; microphone/permission and exact on-device availability on real Windows/Android configurations; actual English/silence/noise/quiet-speech fixtures; browser IME/undo/focus and hardware capture/resampling races; process memory/latency/disk/battery/thermal measurements; real screen-reader/text-scaling/360–390 px and desktop visual QA. Older Android/API 31–32 and absent-service cases still report Unavailable; JNI fallback is unresolved. Initial Android capability probing uses an English US intent; other English-only service configurations need explicit device fixtures. No South African accent/language or punctuation/identifier accuracy claim is made. The code is source-complete in this lane, not a declared accepted or released Phase 6 feature.

## Physical-device support-query correction

The phone granted microphone permission, but candidate `3a11cf6` still returned no installed English languages and `language_unverified`. Samsung Voice Input reports its English (United States) pack at Latest version; the configured Android on-device recognizer is Google AS/AiAi. These are different services. That observation does not establish missing English data or readiness for the recognizer Tjommie uses.

The follow-up preserves distinct support-query outcomes: an unsupported query (14), unsupported language (12), currently unavailable language (13), a successful empty installed-English list, timeout and other failures. Timeout/error/lifecycle invalidation clears cached installed languages. Settings displays the fixed, sanitized native diagnostic. The installed-language list remains query evidence only.

On API 34+, an explicit English (United States) preparation action requests the system-owned model through the explicit on-device recognizer and observes a bounded two-minute callback window. Progress, scheduled download, unsupported callbacks (15), timeout and error never establish readiness. Actual SDK `onSuccess` establishes preparation for that exact intent, separately from the installed-language list. Every subsequent explicit Start under that foreground/page opt-in obtains fresh success on its own capture recognizer before starting audio. Pause, navigation, permission loss and control-scope disposal clear the opt-in. Releasing an old request cannot detach a newer owner; detaching observation never claims to cancel Android's system download. [Android download callback contract](https://developer.android.com/reference/android/speech/ModelDownloadListener), [on-device recognizer API](https://developer.android.com/reference/android/speech/SpeechRecognizer).

This follow-up is source-frozen and independent read-only review reports no blockers. Before freezing, review corrected proof ownership on release, sticky observer-cleanup failure and duplicate terminal callbacks that could remove the recording deadline. The builder's caller batch passed 47/47 tests and pure JVM state checks passed 10/10. Root's combined file/speech integration batch passed 123/123 tests across 14 files, all workspace typechecks and UI lint; 28 generated native source/test files match their authoritative files. New exact-candidate CI and actual preparation/capture checks remain required. Pure JVM state tests do not execute real Android Handler or recognizer-destroy failure paths; neither the Samsung settings screen nor mocked callbacks prove successful dictation.

Candidate `0a22b71` passed both exact CI jobs. Its actual phone microphone check then succeeded through the query-confirmed installed `en-US` path: **Phase 4 files are ready** appeared in the composer and was edited before leaving it unsent. Native status reports available true and permission granted; Android records a completed 4.410-second microphone use. The synthetic host has zero sent user messages, work items and model starts. The SDK model-download action was unnecessary for this successful capture and remains without actual device callback acceptance. This is one spoken-phrase proof, not a noise/accent/quality or all-device claim; see `acceptance-20261103.md` for exact artifact/hash and limits.
