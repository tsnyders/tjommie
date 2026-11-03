# Phase 4 P0: files contract and feasibility

Date: 2026-11-03. Baseline: `aaaf09fe6c45cce2fe42bf9488ac3815156f4ad9`, isolated `phase4-workspace-20261103` worktree. This record covers files only. The owner accepted Phase 3 and the Phase 4 plan; neither acceptance is reopened here.

Decision: retain the 200 MiB share limit, use durable registered copies and 256 KiB base64 RPC reads over the existing channel, and use app-owned native streams for preview/download. Source implementation may proceed under the coordinator's release. Native PDF, real Save As/system picker and Android gates remain open; this is not a readiness or release claim.

## Existing behavior checked

- `team.ts:fileInside/snapshotFile` resolves allowed roots, opens once, compares identity, rejects source growth/mutation, and copies through a 64 KiB buffer. `share_file` already publishes a separate copy rather than the mutable project source.
- Standalone shares immediately move to a bot inbox. Exchange shares remain in `shared-files/share-*`, are copied to the recipient inbox when the reply settles, and update file cards/room post paths. Failure can retain staging. The startup 24-hour staging sweep does not check published message ownership. New durable registration must precede publication and backfill must precede this sweep.
- `FileData` is currently `{path,name,size}`; `ExchangePostData.files` stores names/paths. `MessageView` and `RoomPost` both call `FileCard`, whose Download icon actually invokes the local Show in folder action. `ExchangeView` receives file messages through `MessageView`.
- The current Markdown renderer already rejects raw HTML, blocks active URLs and avoids loading images. Extend its file rendering options; retain chat defaults.
- `EncryptedMessages` has a 16 MiB application limit, 65,535-byte Noise records, and synchronous ordered emission of all records for one application message. A 256 KiB base64 response fits in six records. A large attachment must never become one application message or IPC payload.
- The dispatcher originally discarded connection/device identity. The coordinator now owns a trusted `RequestContext` with a unique `connectionId`, a local/device principal, and an `AbortSignal`, carried from WS/Noise and aborted on disconnect/revoke. Attachment methods reject an absent context. Client parameters cannot manufacture it.
- Desktop has a sandboxed/context-isolated app window and separate bot-browser sessions. Its existing `app:` scheme lacks `stream: true` and its CSP disallows frames. Use a scoped handler in the trusted app session; do not add the handler to bot-browser sessions or bypass CSP.
- Android's source `StaticWebViewClient` already extends Capacitor's `BridgeWebViewClient`. It is the seam for app-owned range responses. Source plugins live under `packages/mobile/native`; generated Android copies are not the source of truth.

## Executed evidence

Commands use disposable temporary roots and synthetic bytes; no owner data, profiles, phone, credentials, provider calls, release builds or packaging were used.

| Check | Observed result | Limit of the evidence |
| --- | --- | --- |
| `node scripts/phase4-spikes/files/transfer.mjs` | Actual compiled Snow WASM plus current `EncryptedMessages`: 200 MiB, 800 reads of 256 KiB, identical SHA-256, six records per response, 279,913,169 length-prefixed wire bytes, 33.473% overhead; initial run 8.701 seconds. Synthetic control messages interleaved at 800 RPC boundaries. | One Node process, no TCP/relay latency or production authorization. The small authorization service is explicitly a contract prototype. No chat/Stop latency guarantee follows. |
| Transfer process memory samples | Initial RSS 76,349,440 bytes; peak 210,935,808. Array buffers initially 638,364; peak 73,385,439. | GC and allocator behavior are included; these are not renderer/native-memory measurements or a fixed memory ceiling. No full-file byte array is used. |
| `npm.cmd exec vitest run packages/daemon/src/team.paths.test.ts packages/daemon/src/team.snapshots.test.ts packages/client/src/encrypted-channel.test.ts -- --maxWorkers=2` | 25/25 tests, 3 files, passed. | Existing source tests and mocked framing isolation where the suite says so; not Android/installed-app proof. |
| `node scripts/phase4-spikes/files/native-run.mjs` | Electron 44.4.5 / Chromium 152.0.7977.130 on Windows. WAV sought to 20/30 seconds. H.264/AAC MP4 sought to 9/12 seconds, 320x180. A valid trailing `free` atom padded the 1,031,298-byte MP4 to 200 MiB; only 4,718,592 bytes were served across three range requests. 256 KiB maximum read buffer. Suffix range 206, invalid range 416, unknown attachment 404, post-revoke 403. Native 200 MiB staged save hash matched; cancel/corrupt cases preserved an existing destination and removed partials. Command deliberately remains nonzero for the PDF gate. | Synthetic padding is not a 200 MiB encoded movie. A standalone preview app/session, not the native bot-browser WebContentsView, production attachment integration, a Save As dialog or Android. |
| PDF native viewer attempts | The 200 MiB PDF can reach PDF.js range loading, but attempts waiting for `textlayerrendered` timed out, including an explicit `pagesinit`/`update()` attempt. A later text-extraction attempt also timed out. | Full reader, text selection and search acceptance remain red. The fixture exits nonzero when its PDF gate is red. No replacement/mock pass is claimed. |

The transfer probe asserts cross-session/device/host refusal, revoked/disconnected contexts, missing message/attachment references, stale revision, two-active-transfer limit, concurrent-read refusal, malformed/out-of-bounds ranges, EOF, HTTP suffix/open-ended range behavior and incremental completion digest. It uses the real Noise implementation, not a cipher double. Final repeated measurements may vary with GC/CPU activity.

Final hidden PDF attempt served 16,252,928 bytes in 63 requests before its 22-second PDF deadline, with no range exception recorded; it did not finish text extraction/viewer rendering. The fixture's renderer private memory was 280,468 KiB (working set 144,856 KiB) at the sampled point. This supports the warning about large PDF allocation, but is not a measured peak or a diagnosis of the render timeout. No more native viewer retries were used as a substitute for an actual visible-app/device gate.

The later source audit found a spike-adapter defect: pinned PDF.js 6.5.438's `PDFDataTransportStreamRangeReader._enqueue` consumes one complete requested range, not multiple fragments. The candidate and harness were corrected to assemble one bounded requested range from <=256 KiB reads and call `onDataRange` once. The recorded native PDF failure predates that correction; the corrected native PDF fixture has not been rerun. Do not attribute the old timeout to a proven platform limitation or convert the correction into a passing native claim.

## API contract

Use a new `packages/protocol/src/attachments.ts` module. The coordinator adds exports/methods and performs one coordinated protocol-version change with the other Phase 4 contracts. Existing strict version checks remain; mixed-version support is not promised.

References are `{messageId, attachmentIndex}`. `attachmentIndex` is zero for a file message, or the persisted position within a room/exchange post. Add optional `attachmentId` when the new card has one, and reject a mismatch. No read API accepts a path, URL, storage name, device ID or connection ID.

| Method | Params | Result |
| --- | --- | --- |
| `attachments.metadata` | reference | `{id,hostId,name,size,contentType,sha256}` |
| `attachments.open` | reference | `{transferId,attachment,chunkBytes:262144,expiresAt}` |
| `attachments.read` | `{transferId,offset,length}`; safe integers, `0 <= offset <= size`, `1 <= length <= 262144` | `{transferId,revision:sha256,offset,bytes:base64,eof}`; exact clamped byte count |
| `attachments.close` | `{transferId}` | `{}`; idempotent for this connection, never closes another connection's handle |

The service is configured with the owning host ID, database and protected managed root. Every metadata/open/read re-resolves the message-to-attachment association. Each transfer belongs to the trusted context's exact `connectionId` and principal. Recheck its signal and device revocation before reading and before returning bytes. A reconnect gets fresh handles; a leaked handle cannot be used by another connection, even for the same paired device.

Starting resource bounds: two active handles per device (two per local connection), one outstanding read per handle, a host cap of sixteen handles, and a renewable five-minute idle lease. Close/abort/timeout releases the file descriptor. Reads use asynchronous file I/O and yield between chunks so control RPCs can run. Bound duplicate queued requests rather than building an unbounded transfer queue. Revocation/disconnect/host switch abort active native read/save/preview state; saved user copies remain.

Errors need stable UI categories for unavailable/missing file, forbidden/revoked handle, corrupt content, expired handle, busy/resource limit, malformed range and unsupported preview. Use validated protocol errors rather than exposing an OS error/path. Offline host is a connection state, not a fake successful RPC. Cancelled native saves resolve `{status:'cancelled'}` rather than success.

## Persistent storage and exact SQL requested from root

Keep durable copies in `<data root>/shared-files/attachments/<uuid>.bin`, outside inbox and staging cleanup. `shared-files` must retain existing protected-root checks/ACL setup. The service validates real root/parent identity and refuses symbolic links; random storage names come from the daemon, never original filenames. Retain the original filename only as metadata. Hash and detect type while bounded-copying to a unique temporary file, then rename and insert the record before announcing the message. Do not hard-link mutable inbox files.

Append a migration at the actual next number (root owns numbering):

```sql
CREATE TABLE attachments (
  id TEXT PRIMARY KEY,
  host_id TEXT NOT NULL,
  storage_name TEXT NOT NULL UNIQUE,
  original_name TEXT NOT NULL,
  byte_length INTEGER NOT NULL CHECK(byte_length >= 0 AND byte_length <= 209715200),
  content_type TEXT NOT NULL,
  sha256 TEXT NOT NULL,
  created_at TEXT NOT NULL
);
CREATE TABLE message_attachments (
  message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
  attachment_index INTEGER NOT NULL CHECK(attachment_index >= 0),
  attachment_id TEXT NOT NULL REFERENCES attachments(id),
  PRIMARY KEY(message_id, attachment_index)
);
CREATE INDEX message_attachments_by_attachment ON message_attachments(attachment_id);
```

Planned daemon surface: `new Attachments({db, root, hostId})`, `registerSnapshot(path, name)` returning metadata, `link(messageId, index, attachmentId)`, `backfillLegacy()`, `cleanup()`, `close()`, and `attachmentHandlers(service)`. `registerSnapshot` accepts only the existing protected snapshot staging tree; it is an internal integration seam, never RPC input. Root's `Team` wiring calls registration before `#add`, links in the same message transaction and includes attachment identity in each file/room copy. Exchange inbox copying for bot context continues independently; durable preview ownership never depends on a later inbox move. A failed message transaction leaves an unreferenced managed orphan that cleanup can remove, never a card pointing to a deleted staging file.

Legacy policy: parse only stored server messages, never a client-supplied path. Register `kind=file` paths and `ExchangePostData.files` positions only when the existing file is a regular non-link inside a known bot inbox or the protected `share-*` snapshot tree. Do not adopt a project path, general bot home, arbitrary absolute path, traversal, linked parent or a stale missing target. Keep stale records readable as Unavailable. Run eager validation/backfill before the old staging sweep so retained published snapshots survive restart. Lazy metadata lookup can retry validated legacy registration. Never delete legacy inbox files after adoption.

The join table defines message ownership. Multiple message presentations may share one immutable record. Deleting a message removes its association; delete a managed copy only when no association and no active lease remain. Closing a preview alone does not delete delivered bytes. A deleted message invalidates further reads even while cleanup waits for a lease. Startup orphan cleanup only considers managed UUID storage files, never arbitrary inbox/project files.

## Native preview and save contract

Desktop: scoped unguessable preview handles in the app's session, with `stream: true`, `supportFetchAPI: true`, secure/standard privileges and no CSP bypass. A handle binds the selected host generation, attachment/revision and renderer/frame owner. Handle lookup supplies bounded RPC reads; URLs contain no path, credential or daemon token. GET/HEAD only, `Cache-Control: no-store`, `X-Content-Type-Options: nosniff`, byte ranges, accurate 200/206/416 responses and cancellation/backpressure. HTTP open-ended/suffix ranges are accepted, while each underlying RPC remains <=256 KiB. Refuse multipart/invalid/unsafe-integer ranges. Native browser sessions receive no attachment handler.

Desktop Save As: trusted main process picks the destination; the renderer receives an opaque save ID only. Create a random exclusive `.part` in the selected destination directory, sequentially append bounded chunks, verify expected byte count and SHA-256, flush/close, then rename to the selected name. On cancel/error/digest failure, remove the partial and preserve the pre-existing destination. No success before finalization; no original source mutation. The native module's stage/append/finish/cancel calls are bound to the trusted renderer and host generation.

Android: an app-owned path intercepted by `StaticWebViewClient`, with opaque per-page handles and `WebResourceResponse` 200/206/416 semantics. A bounded InputStream requests <=256 KiB through the native/JS bridge; native waits on a background thread with cancellation/timeouts, not the WebView UI thread. JS still obtains bytes over Noise; no public HTTP listener or arbitrary native file reader. Invalidate handles on page generation/background/host switch/revoke, abort queued reads, and retain at most the explicit small stream buffer. Actual WebView range/seek remains a required device gate.

Android first fetches metadata, then opens `ACTION_CREATE_DOCUMENT` before acquiring a transfer or staging bytes. The pending picker is a local destination intent. After it returns, the app must obtain a current authenticated client for the same selected-host epoch, reopen the original message reference and compare every metadata field before activating native staging. From the fresh open onward, disconnect/revoke/host switch cancels the transfer; it cannot rebind to another client. Ordinary transport loss while choosing a destination may preserve only that byte-free local intent. Host change (including A to B to A), navigation, unmount, refusal/revocation and cancel invalidate it. The app stages and verifies bounded chunks while visible, then copies to the selected URI and reads it back to verify its digest. Picker cancellation reports cancelled. Failure after document creation attempts `DocumentsContract.deleteDocument`, retaining a recovery reference if cleanup fails. Arbitrary document providers do not promise atomic commit/rename/delete: do not claim that every provider can guarantee removal after crash or write failure. This is an explicit P0 contract limitation requiring native/provider acceptance, not a reason to silently report success or add broad storage permission. System providers may represent cloud destinations; saving there is the user's picker action, not automatic upload by Tjommie.

## Preview formats and PDF dependency

Candidate tested set to establish in P2: UTF-8 Markdown with GFM; PNG/JPEG/WebP/GIF raster; PDF; PCM WAV and MP3 audio; H.264/AAC MP4 and VP8/Opus WebM video where the current platform decoder confirms support. This P0 does not establish every format on Windows or Android. Reject mismatched signatures, invalid UTF-8, corrupt media and active formats for preview; Download remains available. SVG/HTML/executables are download-only, never auto-opened. AAC/MP3/AV1/HEVC and other codecs need actual fixtures per platform before any support claim; `canPlayType` alone is not playback proof.

Markdown relative images resolve only through explicitly associated shared attachment IDs and normalized companion names. No disk crawl, remote resource fetch, `file:`/`javascript:`/`data:` image escape, raw HTML or script execution. Anchors remain within the viewer. External HTTP(S) links require a deliberate user action through the existing external-link policy.

Select exact **`pdfjs-dist@6.5.438`**, Apache-2.0, verified against Mozilla's release and npm metadata. Use the display/viewer components and local worker, not the generic hosted viewer or a new React wrapper. Construct no `PDFScriptingManager`; set XFA/forms/annotation editing inactive, control external links, prohibit attachments/launch/JS actions and unnecessary viewer controls. Password input is session-only and cleared on close; password fixture behavior is still unverified. Do not add `isEvalSupported` based on older advice: this exact distribution has no such option.

Use explicit `PDFDataRangeTransport` for app-owned schemes, avoiding assumptions about HTTP-only automatic range discovery. Split any larger PDF request into bounded RPC reads and feed the pinned transport's supported interface. Set `disableStream:true`, `disableAutoFetch:true`, `rangeChunkSize:262144`; explicitly cap visible canvas count/pixel budget and destroy the worker/document on close. Those flags reduce reads, not all PDF memory: PDF.js `ChunkedStream` allocates an array with the full document length. Searching every page can fetch more content. Do not promise a small constant-memory 200 MiB PDF reader or silently impose an unapproved PDF size cap. Large-document resource policy and native measured behavior remain an acceptance decision.

Coordinator-installed disposable package: `scripts/phase4-spikes/files/runtime/node_modules/pdfjs-dist`, with no production manifest/lock edit. Full npm package unpacked size: 37,616,588 bytes. Selected local runtime assets, excluding source maps:

| Asset | Bytes | Individually gzipped bytes (measurement only) |
| --- | ---: | ---: |
| `build/pdf.mjs` | 891,540 | 184,974 |
| `build/pdf.worker.mjs` | 2,248,830 | 484,423 |
| `web/pdf_viewer.mjs` | 322,241 | 72,246 |
| `web/pdf_viewer.css` | 168,800 | 25,694 |
| `cmaps` (169 files) | 1,167,747 | 982,956 |
| `standard_fonts` (16 files) | 798,046 | 548,958 |
| `wasm` (15 files) | 1,388,487 | 511,893 |

Also preserve package and font/WASM notices and CSS image resources actually referenced by the selected viewer. Do not ship Node's optional `@napi-rs/canvas` native package into the UI; the browser build excludes it. Root owns the exact manifest pin/lock, allowlisted asset-copy step, static export, CSP/worker and release inclusion. No new transfer, HTTP server, image, media or Markdown dependency is justified.

## Implementation allowlist and sequencing

Files lane source candidates, after P0 handoff:

- Protocol: new `packages/protocol/src/attachments.ts` plus its contract test.
- Daemon: new `attachments.ts`, `attachments.test.ts`, `attachment-handlers.ts`, `attachment-handlers.test.ts`.
- UI: `components/chat/file-card.tsx` and its test; new `file-viewer.tsx`/test and `lib/attachment-transfer.ts`/test; file-card caller changes in `chat/message-view.tsx` and `room/room-post.tsx` plus affected tests. Coordinate the exchange caller through the voice lane: do not edit its owned composer/view file concurrently.
- Desktop: new `attachment-save.ts`/test and `attachment-protocol.ts`/test.
- Android: new native source attachment stream/save/plugin modules and JVM tests; new typed attachment wrapper/source tests under `packages/mobile/src`.
- Evidence: this record and `scripts/phase4-spikes/files/**` only. Runtime `node_modules` is disposable/ignored.

Root-owned integration files: `protocol/model.ts`, `wire.ts`, `index.ts`, protocol version; `db.ts` append-only migration; `request-context.ts`, `server.ts`, `remote-host.ts`, `daemon.ts`, `team.ts`, protected-root setup; desktop `main.ts`, `preload.cjs`, UI `DesktopApi`, shared connection/host-generation lifecycle; Android `MainActivity`, `StaticWebViewClient` integration and `apply-native.mjs`/manifest; `prepare-ui.ts`, package/lock/CSP, notices/release fixtures and CHANGELOG. Files lane supplies narrow wiring snippets and no edits to these unless reassigned. Voice owns common composers; teams owns its own orchestration lane.

Order: (1) protocol/storage/authorization tests and root migration/context wiring; (2) snapshot registration/backfill/retention integration; (3) native staged saves/scoped streaming with cancel/revoke lifecycle; (4) card references and all conversation presentations; (5) format viewers and local PDF assets; (6) Windows + Android native gates and compatibility/integration checks. P1/P2 cannot be marked accepted from source tests alone.

Remaining gates: actual current-device Save As; Android picker/provider cleanup and WebView media/PDF ranges; PDF native text selection/search/password and resource behavior; supported raster/codec fixtures; two-client cross-host/revoke races and production chat/approval/Stop coexistence; responsive/focus/screen-reader viewer UX; integration migration/legacy cleanup and packaged asset proof in GitHub Actions. Owner installation/release work remains separately authorized.

## Primary references

- [Mozilla PDF.js 6.5.438 release](https://github.com/mozilla/pdf.js/releases/tag/v6.5.438)
- [PDF.js API options](https://mozilla.github.io/pdf.js/api/draft/module-pdfjsLib.html)
- [Pinned PDF.js ChunkedStream allocation](https://github.com/mozilla/pdf.js/blob/v6.5.438/src/core/chunked_stream.js#L25-L35)
- [Electron protocol streaming privileges](https://www.electronjs.org/docs/latest/api/protocol)
- [Android document picker and provider storage](https://developer.android.com/training/data-storage/shared/documents-files)

## P1/P2 source candidate handoff (not acceptance)

The coordinator released the files lane for source implementation after the P0 handoff. Owned source now includes protocol schemas, daemon storage/authorization/handlers, desktop stream/save/bounded-read broker, native Android stream/staging/plugin plus typed wrapper, client transfer validation, file cards/callers, Markdown safety options and image/PDF/media viewer components. Native browser capture is a separate work package and gains no readiness claim from the standalone preview fixture.

The daemon unit tests explicitly install a disposable schema fixture because the production migration belongs to root. Passing those tests does not establish that an upgraded installed database has the tables. Root still owns the actual migration, protocol index/method/error wiring, daemon/team registration and native integration. Before that wiring, root/UI TypeScript checks report the missing shared contracts; the source candidate is not integrated or shippable.

Registration hashes/fsyncs the managed copy before publication. A transfer's `open` returns a handle after FD/stat checks; its first read verifies the complete digest before releasing bytes, checking the trusted context/closed flag between 64 KiB reads. This placement lets `attachments.close` cancel a pending integrity scan without closing the whole chat connection. Opening/closing FDs remain counted against transfer capacity until their actual close completes.

Current native API in `protocol/src/attachments.ts`:

```ts
type AttachmentNativeApi = {
  startSave(info: AttachmentInfo): Promise<{ saveId: string } | null>;
  activateSave?(saveId: string): Promise<void>;
  appendSave(saveId: string, offset: number, bytes: string): Promise<void>;
  finishSave(saveId: string): Promise<{ status: "saved" | "cancelled" }>;
  cancelSave(saveId: string): Promise<void>;
  preview(info: AttachmentInfo): Promise<{ previewId: string; url: string }>;
  onRead(listener: (request: AttachmentNativeRead) => void): () => void;
  respond(previewId: string, requestId: string, bytes: string | null): Promise<void>;
  closePreview(previewId: string): Promise<void>;
  invalidate(options?: { scope: "transfers" }): Promise<void>;
};
```

`AttachmentNativeRead` contains `previewId`, `requestId`, `offset`, `length`. The client captures one `Client`, validates transfer ID/revision/offset/exact byte count/canonical base64, and forwards only <=256 KiB chunks. It discards late opens after cancel and never finalizes a save after an abort wins the start-reply race. Downloads are hashed/finalized by native storage, with no whole-file renderer Blob. Preview URL validation requires the expected app origin, exact opaque UUID handle, no query/fragment/user info, and the same `previewId`.

Root integration checklist, with exact seams:

1. Export `attachments.ts`; spread `attachmentMethods` into protocol methods; add `forbidden`, `unavailable`, `expired`, `corrupt` errors; include the change in the coordinated protocol version. Extend existing strict `FileData` with optional `attachmentId: z.uuid()` and `contentType: z.string()`, and `ExchangePostData.files` with optional attachment ID, detected content type and byte size, so old records still parse. Publish these values from registered metadata so every card can show actual type/size. Add the SQL above as an actual append-only migration and test upgrade/rollback behavior.
2. Configure `Attachments({db,root:paths.root,hostId})` using the stable owning host identity, not a common string such as `local`. Ensure this identity is available without contacting a provider. Spread `attachmentHandlers(service)` into daemon handlers, call `backfillLegacy()` before `team.start()`, and close the service before DB shutdown. Pass the trusted RequestContext only from authenticated transports; internal dispatch remains unprivileged.
3. `Team` registration: call `registerSnapshot(snapshot.path,snapshot.name)` before publishing a new file message; include returned `id` as `attachmentId`; call `link(message.id,0,id)` in the same message transaction. A central `#add` hook can link file and room-post attachment IDs without duplicating ownership logic. Preserve the ID when inbox paths change. Build room/exchange copies using that original ID and link each room message/index to it, carrying registered size/content type as well. Active task attempts must resolve `share_file` paths against their recorded worktree context (coordinate the teams lane), not the bot's unrelated project folder. The snapshot/attachment gate itself still accepts no arbitrary task path from a client. If the publish transaction fails, cleanup may remove the now-unreferenced managed copy, never a referenced snapshot. Bot/room/message deletion calls `cleanup()` after ownership cascades; the lease keeps an unreferenced copy until transfer close. Do not remove legacy inbox files.
4. Desktop `AttachmentSaves.start(owner,info,nativeSelectedPath,signal)` receives a Save As result only from main. A cancelled dialog returns `null`. Route `append/finish/cancel` by trusted sender owner, never renderer-selected filesystem path. `AttachmentProtocol.create(owner,info,read,signal)` returns `{handle,url}`; its `read` uses `AttachmentReadRequests.request(owner,handle,offset,length,emit)`. Emit the resulting native read request only to that renderer. `respond(owner,previewId,requestId,bytes)` validates it. Closing/invalidation cancels broker requests, scoped protocol handles and save staging together.
5. Register `stream:true` for the existing app scheme, and route only `/_attachments/<uuid>` through the attachment protocol before static UI serving. Keep the handler in the trusted app session, absent from bot browser profiles. Bind owner to trusted frame/window plus selected-host generation; invalidate on host switch, disconnect, revoke/window destruction, and before granting a new generation. Expose only `AttachmentNativeApi` in preload and optional `DesktopApi.attachments`. Reject raw-content navigation/downloads outside this explicit save flow; raw responses carry a sandbox CSP and safe MIME allowlist.
6. Android: register `TjommieAttachmentPlugin` in source `MainActivity`. `StaticWebViewClient` must retain the constructor's `Bridge` in its own field (the superclass field is private and has no getter). At the beginning of `shouldInterceptRequest`, call `TjommieAttachmentPlugin.intercept(thatBridge,request)` and return non-null responses before static routing. Register `TjommieAttachments` and adapt with `attachmentApi`. Native source copies/JVM tests follow normal native sync; no generated Android copy was edited here. The plugin already fences page generations, backgrounding and late replies and retains only private staged files until verification/picker/copy.
7. `attachmentDevice()` selects the current device's desktop bridge or Capacitor plugin; it deliberately does not use the old local-host-only reveal gate for Download. Add `invalidate()` to the shared connection's host-switch/disconnect lifecycle before old client state is discarded. `MessageView` and `RoomPost` now pass exact message/index refs; no voice-owned composer or exchange-view file was edited. Exchange rendering via `MessageView` inherits the file-card change.
8. Root provider instructions in `bots.ts` must require `share_file` for each generated deliverable rather than a raw host-path-only final reply, with synthetic fixtures in both Claude/Codex acceptance sources. These paid provider suites were not run.
9. Pin `pdfjs-dist` **6.5.438** in the UI via root's manifest/lock change and copy local `build/pdf.mjs`, `build/pdf.worker.mjs`, `web/pdf_viewer.css`, CMaps, standard fonts, WASM and their notices under `public/pdfjs`. The source reader uses display/TextLayer directly; it does not require `web/pdf_viewer.mjs` at runtime. These measured groups total **6,663,450 bytes** before additional notices/CSS image resources. Preserve local worker/CMap/font/WASM paths. Add the stylesheet to the app's global CSS (`@import url('/pdfjs/web/pdf_viewer.css')` before other rules, or a tested equivalent). The component does not inject a manual stylesheet tag. Coordinate CSP/worker asset packaging and verify static export in GitHub Actions; do not add a hosted reader or Node canvas to UI assets.
10. Add the user-visible CHANGELOG entry and integration/package assertions only after shared wiring is coherent. Rerun the affected unit/contract suites and root/UI/mobile typechecks/lint/no-any, then real Windows/Android acceptance. No builds, packages, commits, pushes or owner installation were performed by this lane.

The source candidate makes **provisional, unaccepted resource limits** visible: Markdown preview refuses more than 2 MiB before fetching text; PDF refuses an individual requested range larger than 2 MiB or more than eight queued range requests, with a complete Download fallback. PDF ranges are processed serially, so only one bounded range buffer is assembled at a time. These policies avoid an unbounded renderer string/DOM or a whole-file range workaround. They are not claimed to fulfill every accepted-size preview; accepting the limits or implementing a different large-document path remains a product/implementation decision. PDF also uses one capped canvas, but the upstream full-document worker allocation remains. Native PDF text selection/search/password, packaged worker assets and full hardware behavior remain unaccepted.

Markdown supports safe anchors and an explicit local companion-image resolver, but the viewer currently supplies no automatic companion associations. Images with no explicit shared-resource mapping stay alt text; no directory crawling or remote fetching occurs. Publishing/associating companion images remains open. Legacy records have no historical digest: matching-size validated legacy files are registered from their currently retained bytes, not falsely proven identical to a pre-Phase-4 original; changed recorded sizes become Unavailable.

Validation at final source handoff: **49/49 focused Vitest assertions across nine files passed**. The separate caller/host-gate regression run passed **24/24** (overlaps some focused tests; do not sum them). Java core JUnit: **4/4** using only disposable compiled test classes and existing JUnit dependencies (`node scripts/phase4-spikes/files/jvm-selfcheck.mjs`); no Android plugin/app compilation or hardware proof. Scoped UI ESLint passed without warnings; the no-any scan passed across **527 TypeScript files**. Tracked owned UI paths passed `git diff --check`; no commit/push occurred and HEAD remains the source baseline. Shared integration/typecheck gates remain open until root wiring lands.

## Remaining files implementation, 2026-11-03

The later source increment replaces the provisional Markdown/PDF restrictions above. `share_file` accepts an explicit bounded list of companion image names and owned source paths. Names are normalized and reject traversal, URLs, host paths and double encoding. Every source uses the existing checked snapshot path; publication is atomic. Main cards persist companion IDs and message indexes; room copies append companion indexes after their unchanged primary file indexes. The existing ownership table and leases protect all copies without a migration. No directory crawling or remote image fetch is added. The viewer opens one explicitly selected inline companion at a time, closes it on section change/unmount, and closes both companion/document before starting Download under the existing two-transfer budget.

Markdown uses 32 KiB UTF-8 sections, including first/last access at 200 MiB and bounded complete-file search. UTF-8 boundary characters are preserved. The UI identifies section continuation; a Markdown block may span sections. The full document is never assembled as one renderer string or DOM.

`scripts/pdfjs-worker.ts` adapts the exact SHA-256-pinned PDF.js 6.5.438 worker's contiguous range grouping to individual 256 KiB requests. Asset preparation fails closed if upstream bytes change and preserves a prominent modified-source notice. The renderer serializes up to 800 lightweight pending requests, returns one exact callback per range and rejects overlap; it does not assemble a larger range. One visible canvas remains capped at 8 Mi pixels, page resources are cleaned after rendering/search and the worker is destroyed on close. Upstream `ChunkedStream` still allocates `new Uint8Array(document.length)` in the worker: a 200 MiB file requires that allocation plus PDF parsing/decoding. This is not constant-memory reading or a calibrated low-memory Android guarantee.

Executed `node scripts/phase4-spikes/files/large-document-run.mjs`: **passed**, using the actual React viewer components, a modified local pinned worker and generated documents in a disposable hidden/offscreen Electron 44.4.5 / Chromium 152.0.7977.130 profile. A 200 MiB PDF with a real 3 MiB page stream rendered/selectable text, searched to page 2 and zoomed to 1.25; 14 reads served 3,670,016 bytes. The 200 MiB Markdown fixture opened first/last of 6,400 sections with 65,539 bytes, 32,765 rendered characters, displayed the explicit image and closed its lease on navigation. Maximum underlying read was 262,144 bytes; one read was active. Renderer private memory samples: initial 22,128 KiB, peak 271,400 KiB, final 71,796 KiB; final JS heap 19,147,044 bytes. Samples are observations, not a memory ceiling. The first fixture placed padding as whitespace before xref, provoking linear lexer reads; the corrected fixture puts padding in a valid unreferenced stream. No platform limitation or production success is inferred from that earlier timeout.

Final affected regression run: **109/109 across 17 files passed**, 30.32 seconds. Root/UI TypeScript and scoped ESLint passed; no-any passed across 593 TypeScript files; owned tracked whitespace check passed. These checks do not establish installed-app/Noise/Save As/Android hardware, codec, password, screen-reader or viewport acceptance. No app/release build, owner app/data/device, microphone, credentials, commit or push was used.

## Physical Android Save continuation

The foreground-service design below records an intermediate attempt and is superseded by the destination-first correction at the end of this section.

The `5899f8c` phone run exposed an actual native socket failure during the system picker. The existing disconnect fence then cancelled and removed staging as required; no successful saved copy was reported by acceptance. The scoped native trace records an IOException category about 5.5 seconds after `finishSave`; the exact test PID's later freezer event occurs about 70 seconds after picker entry. That later freeze is not evidence for the earlier failure's cause.

A long picker also has a separate definite lifetime problem: the normal hidden-app heartbeat is disabled while the server expires idle peers after 150 seconds. Retaining the accepted disconnect/revoke cancellation contract therefore requires an active, user-initiated save component rather than an offline-finalization exception.

The follow-up uses one non-exported, save-scoped `dataSync` foreground service covering staging, verified picker, provider copy and digest read-back. It starts from the visible Activity before the picker, shows a generic notification with Cancel and stops on terminal outcome, invalidation, navigation, destruction, watchdog or the bounded absolute timeout. It does not enable permanent keep-alive, request notification permission or alter phone-wide battery settings. Android explicitly permits download/export/local-file processing for this service type. [Android foreground service types](https://developer.android.com/develop/background-work/services/fgs/service-types)

Native tick challenges drive only the existing authenticated heartbeat through the captured current JS connection. Acknowledgments bind the save/page generation and the selected host/link; they expose no keys to native code and never rebind an old lease to a new connection. The attachment-host UUID and paired remote-host UUID are different namespaces and must not be compared as equal identities. Shared-file saves still cancel on an actual disconnect or revoke. A generated local diagnostics export uses a separately named trusted local-export factory; a remote metadata `hostId` string cannot select that exception.

The implementation is frozen and independent read-only review has no remaining findings. Focused tests cover promotion, challenge/page binding, the nine-minute picker and absolute limit, irrevocable cancellation, cleanup ordering through destruction, stale notification dispatch and diagnostics unmount/delayed digest. Root verified all 29 generated native source/test copies match their authoritative files. Exact-candidate CI and physical acceptance remain pending. A foreground service is not a guarantee against real network loss. The required physical checks cover the long picker, exact copied hash, cancel/revoke/host-switch and retirement of its service/notification.

### Destination-first correction

The `3a11cf6` service starts correctly and a quick actual Save passes with the exact 216-byte hash. A delayed picker still fails: native challenges at zero, 30 and 60 seconds receive authenticated acknowledgments, while the 90-second challenge produces no native send. The watchdog expires at 105 seconds. A foreground native process does not establish that hidden WebView bridge/crypto work will continue. No supported queue exemption has been established, and global Chromium flags, fake visibility and audio keep-alives are excluded.

The corrected order above removes that dependency. No transfer, FD or file bytes exist while the user chooses a destination. Fresh authentication after selection rechecks device/message/ref authorization; it is not offline finalization. A separate explicit activation prevents stale operations from writing after invalidation. Once fresh open begins, all real disconnect cancellation rules apply. Empty provider-created documents must be removed if authentication, metadata revision, activation or copying fails, retaining the existing truthful cleanup-failure/recovery behavior. Local diagnostics exports follow the same picker/activation ordering with their own mounted-document liveness proof. The unused service, heartbeat machinery and its added foreground-service permissions are removed. If a late picker or detached cancellation cannot delete its created URI, common native cleanup retains recovery and queues a fixed generic warning until the Activity resumes; it exposes no URI, filename or host detail. Independent read-only review reports no blockers. Exact-candidate CI and physical acceptance remain required.
