# Phase 3: Remote Access Implementation Plan

> **For agentic workers:** Implement through Paseo agents, one task per agent in its own worktree, following superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax. Every task gets a read-only review from the other provider. Reviewers report findings and never edit. All git work (commits, merges, pushes, PRs, releases) goes to the ops agent.

**Goal:** A paired phone or second PC reaches the Tjommie host from anywhere: over a relay on mobile data, with push when a bot waits. The phone stays connected without manual reconnects. The desktop app starts at login and updates itself.

**Architecture:**
- A Cloudflare Worker relay with one Durable Object per host. It pairs a host "control" WebSocket, authenticated by an Ed25519 challenge signature, with per-device "channel" WebSockets. It forwards the existing Noise frames verbatim, so the relay never sees plaintext.
- The daemon's `RemoteHost` stops assuming TCP and accepts a generic framed wire, so LAN and relay legs share one handshake and RPC path.
- Clients race the LAN addresses for 1 s, then fall back to the relay. A heartbeat and resume/network-change triggers make reconnects invisible.
- FCM push goes host → relay → Google with no bot content.
- The desktop gets a login item, a `--hidden` start and electron-updater on GitHub Releases.

**Tech stack:**
- Existing: Node 24 daemon (`node:sqlite`, `node:crypto` Ed25519, global `WebSocket`), Snow Noise WASM (`packages/transport`), zod (`packages/protocol`), Electron 44, Next.js static UI, Capacitor 8.5.3.
- New: Cloudflare Workers with SQLite-backed Durable Objects (wrangler), `jose` (FCM service-account JWT in the Worker), `@capacitor/push-notifications` and `electron-updater`.

**Spec:** `docs/superpowers/specs/2026-09-25-tjommie-design.md` §3, §7 (notifications), §9.2, §11 (`devices`), §12.2 (Phase 3 demo), §13, §14.

**On approval:** the planner's only step is to write this plan to `docs/superpowers/plans/2026-10-25-phase3-remote-access.md`, then stop. No product code, no installs, no git.

**Base:** worktree `C:/Users/tyrel/.paseo/worktrees/2cb7zj88/tjommie-phase1-completion`, branch `phase2-accounts-mobile-20261022` @ `81104d4`. The ops agent cuts `phase3-remote-20261025` from it, and each task branches from that.

## Context

Phases 1, 2, 2.5 and team rooms were accepted on 2026-10-25 (PR #3). The phone works only on the same Wi-Fi while the app is in front.

The owner also reports that the phone "automatically disconnects when I leave the application for a while. It needs to stay connected forever." On 2026-10-25 at 03:11 Samsung froze the dozing app and `TjommieSocket.open` failed "unreachable", even though `nc 192.168.0.2 47101` from the phone worked.

Code reading confirms four causes:
- There is no heartbeat. A dead TCP socket looks alive until a write fails, because Android keepalive defaults to about 2 h.
- Nothing reconnects on resume or on a network change.
- `ui/lib/connection.ts:183` puts a phone into the full-screen `failed` state after 3 failed tries, so a few seconds of Doze leaves a "Can't reach or authenticate this PC" screen.
- The JS retry timer is throttled or frozen in the background.

Phase 3 fixes those and adds everything the spec still lists: the relay, push, another PC, always-on and auto-update.

## Owner decisions

**Decided by the owner on 2026-10-25:**
1. **Phone in the background:** both mechanisms.
   - Always: an instant silent reconnect on return or network change, heartbeats and catch-up.
   - Plus an **"Always connected"** toggle, **default ON**. It runs an Android foreground service with a persistent "Connected to <PC>" notification and keeps the link live in the background.
2. **Push:** FCM via the relay, as in the spec.
3. **Public relay:** the owner's Cloudflare account on its default `*.workers.dev` URL. A custom domain can come later as a settings change.
4. **Windows signing:** unsigned for now. SmartScreen warns on first install. Updates are checked against the sha512 in GitHub's `latest.yml` over HTTPS.

**Planner's calls (the owner can overrule):**
5. **Relay registration is trust-on-first-use per hostId.** The Durable Object stores the host's Ed25519 login key on the first valid login. Later logins must match it, so nobody can take over or knock off an existing host. The host logs in before it issues any relay-bearing pairing code. Squatting would need the random hostId before that host ever connected.
6. **Push works only through a relay that holds the matching Firebase credentials.** That is the public relay, or a self-hoster who also builds their own APK with their own Firebase project. The self-host guide says so.
7. **The pairing code moves to `v: 2`** (adds `relay: string | null`), and `RemoteReady` gains `relay`. Old APKs can't pair with the new host, so the Phase 3 APK and desktop ship together.
8. **The push payload has no bot content:** `{v, hostId, kind, botId}` with a fixed title and body per kind. Opaque ids are not content.
9. **LAN endpoints are tried in parallel with a single 1 s window, then the relay.** Trying up to 16 addresses one after another at 1 s each would add up to 16 s on mobile data.
10. **"Always connected" stops when the app is swiped away from Recents.** Push covers that case.
11. **Releases are 0.3.0.** The APK's `versionCode` becomes 3.
12. **A desktop device key lives in Windows Credential Manager,** through the daemon's existing `windowsCredentialTransport` (`daemon/src/account-credentials.ts`), at target `Tjommie/device/<hostId>`.
13. **Start at login defaults ON** at the first packaged run. A toggle is in Settings → General.

**Owner actions (agents never do these):**
- **Cloudflare:** `npx wrangler login`, then `npm run deploy -w @tjommie/relay`. Report the `workers.dev` URL. A luna task then sets `DEFAULT_RELAY_URL`.
- **Firebase:** create a project with Android app `dev.tjommie.app`. Download `google-services.json` and point `TJOMMIE_GOOGLE_SERVICES_JSON` at it outside the repo. Create a service-account key and run `npx wrangler secret put FCM_SERVICE_ACCOUNT`.
- **Android release keystore:** create one and keep it outside the repo. Set `TJOMMIE_ANDROID_KEYSTORE`, `_PASSWORD`, `_KEY_ALIAS` and `_KEY_PASSWORD` when packaging.
  - The installed S25 APK is debug-signed, so the release APK needs an uninstall first. That loses the saved pairing, so re-pair afterwards.
- **Second PC:** say which machine is used for the real "pair a second PC" demo. Agents prove the flow with two isolated daemon profiles on one PC.
- **Phone access:** an explicit OK before any agent uses the S25 (T0 spike, T12 acceptance).
- **Release approval:** approve publishing the GitHub Release (ops agent). 0.3.0 has to be installed by hand once, because earlier builds have no updater.

## Global constraints

- Never use `any` in TypeScript. The `npm run check:no-any` gate stays green.
- No invented crypto:
  - Noise stays Snow `=0.10.0` (`Noise_IK[psk2]_25519_ChaChaPoly_BLAKE2s`).
  - Relay login is a plain Ed25519 signature (`node:crypto` on the host, WebCrypto `Ed25519` in the Worker) over a domain-separated, fresh challenge.
  - FCM auth uses `jose`.
- No console windows: every spawn uses `windowsHide: true`, including test helpers that run `wrangler dev`. The NSIS update runs silent.
- Agents never use real credentials or do real account operations. Firebase, Cloudflare and signing are faked or local: miniflare, a fake FCM endpoint, a throwaway keystore.
- Agents never stop the installed Tjommie, never install over it, and never touch the S25 without an explicit owner OK.
  - Update and packaged tests use a separate appId and productName with isolated data roots.
  - Phone tests use a separate `applicationId` unless the owner OKs the real app.
- The native browser stays desktop-only. Any remote client, including a second PC, shows the honest "Runs on your PC" state (`ui/components/chat/bot-panel.tsx:160-171`).
- Every paired device keeps full owner authority (spec §9.2.5). The relay adds no scoping.
- Copy follows the existing plain style. Use the spec's exact strings: "Host offline" (shown as "Your PC is offline"), "Can't reach your PC", and "A bot needs you".

## Invariants (security, privacy, logging)

1. **The relay sees only:** IPs, timing, sizes, host ids, channel ids, and for push the device token plus its kind. It **stores** only `{hostId → host login public key}`. It never stores or logs push tokens, frames or IPs. Set `[observability] enabled = false` in `wrangler.toml` and add no `console.*` of request data.
2. **The relay cannot read or forge traffic.** Devices pin the host's Noise static key from the out-of-band pairing code. Pairing over the relay also needs the one-time PSK. Changing the relay URL never changes trust.
3. **Revoke is immediate on every path.** `RemoteHost.revoke` closes LAN and relay peers. A revoked or unknown key then gets an authenticated `refused` after the Noise handshake, and the device stops retrying.
4. **Secrets stay out of events, logs, argv, chat, SQLite and errors.** That covers the host login private key (Credential Manager target `Tjommie/<hash>/relay-login`), device keys, push tokens (stored only in `devices.push_token`), channel ids and the FCM service account.
5. **Push contains no bot content, and no frame is ever decrypted by the relay.**
6. **Reconnecting never repeats a mutation.** In-flight calls reject on loss, as they do today. Catch-up only re-reads.
7. **Pairing codes stay single-use with a 10 min TTL.** The PSK lives only in memory, and the code is consumed atomically (`remote-host.ts:179-183`).

## Review Focus

1. **Phone locked about 30 min (Doze), then reopened.** The UI is live within about 1 s from cached state, with no failed screen, and messages, approvals and bot states from the gap are present. Test: T9 `connection.resume.test.ts`.
2. **Wi-Fi ↔ mobile data switch mid-session.** A zombie socket is replaced at once on the network event, or within 10 s by the pong timeout. The new leg goes over the relay, and an in-flight `chat.send` rejects instead of being sent twice. Test: T9 `connection.heartbeat.test.ts`.
3. **The PC sleeps and wakes.** The relay marks the host offline within 75 s and devices show "Your PC is offline". On wake, the daemon notices timer drift and logs back in within 5 s, and devices reconnect on their own. Tests: T3 `room.test.ts` "host dead after 75s", T5 `relay-client.test.ts` "drift triggers relogin".
4. **A revoked device reconnects over LAN or the relay.** It gets an authenticated `refused` → `removed` state, with no retry loop. A stranger key gets the same refusal and learns nothing. Test: T4 `remote-host.test.ts` "refused after revoke".
5. **Relay abuse.**
   - A different key logging in for a registered hostId → 4401, and the original stays online.
   - A >65,535-byte message closes that leg.
   - More than 20 connects per IP per minute are refused.
   - A push with >16 tokens or an unknown kind is rejected.
   - Test: T3 `room.test.ts`.

---

## Already built (verified 2026-10-25, not re-planned)

- **LAN pairing:**
  - QR plus pasted code. `RemoteHost.createPairing` is at `daemon/src/remote-host.ts:74-87`.
  - The code format is `PairingCode` in `protocol/src/remote.ts:9-12`, with `relay: z.null()`.
- **Noise and framing:**
  - Snow IK/IKpsk2 via `packages/transport/src/index.ts`, with prologue binding.
  - Chunked encrypted messages and length-prefixed frames in `client/src/encrypted-channel.ts`.
  - `authenticateRemote` and `FrameTransport` are in `client/src/remote-peer.ts`.
- **Devices:**
  - The table is `db.ts:209-216` and the store is `devices-store.ts`.
  - `devices.*`, `pairing.*` and `remote.status` are wired at `daemon.ts:274-281`.
  - Revoke closes live peers (`remote-host.ts:96-100`). Every remote request goes through the same `server.dispatch` as local requests.
- **Host identity:** the Noise key is in Credential Manager (`remote-identity.ts`), and `hostId` is a random UUID.
- **Android:**
  - Plugins: `TjommieSocket` (raw TCP, `FrameCodec` 65,535), `TjommieSecureStore` (Keystore AES-GCM), `TjommiePairingScanner` (Play code scanner) and `TjommieAppearance`.
  - Phone client: `ui/lib/mobile.ts`. Saved hosts are in localStorage `tjommie.mobile.hosts.v1`, at most 16.
- **Desktop:**
  - Tray with Open/Quit, hide on close, and a single-instance lock (`desktop/src/main.ts:70-96, 428-466`).
  - The daemon is a child process with `--token-stdin` (`daemon-process.ts`). Quit stops both.
  - Desktop notifications use `listenForNotifications` (`main.ts:251-299`).
- **UI:** Devices section, pair dialog and phone connect screen (`ui/components/devices/*`, `mobile-connect.tsx`). Connection states are in `ui/lib/state.ts:13-19`.

## Frozen contracts (T1 lands these before any wave-1 task starts)

### Protocol (`packages/protocol/src/remote.ts`, new `packages/protocol/src/relay.ts`)

```ts
// remote.ts
export const RelayUrl = z.url({ protocol: /^https$/ }).max(512);          // base URL, e.g. https://tjommie-relay.x.workers.dev
export const PairingCode = z.object({ v: z.literal(2), relay: RelayUrl.nullable(), /* rest unchanged from v1 */ }).strict();
export const RemoteReady   = z.object({ type: z.literal("ready"), deviceId, hostId, hostName, relay: RelayUrl.nullable() }).strict();
export const RemoteRefused = z.object({ type: z.literal("refused"), reason: z.enum(["revoked", "unknown"]) }).strict();
export const RelayState = z.enum(["off", "connecting", "online", "error"]);
export const RemoteStatus = /* existing */ .extend({ relay: z.object({ url: RelayUrl.nullable(), state: RelayState, push: z.boolean() }) });
remoteMethods += {
  "remote.ping":          { params: z.object({}), result: z.object({}) },
  "device.registerPush":  { params: z.object({ token: z.string().min(1).max(4096).nullable() }), result: z.object({}) },
};
// model.ts Settings
relayUrl: RelayUrl.nullable()   // DEFAULTS.relayUrl = DEFAULT_RELAY_URL
```

```ts
// relay.ts: shared by daemon and Worker
export const DEFAULT_RELAY_URL: string | null = null;  // set to the owner's workers.dev URL after deploy
export const RELAY = { maxFrame: 65_535, hostPingMs: 30_000, hostDeadMs: 75_000, acceptTimeoutMs: 10_000,
  maxChannels: 32, maxPendingChannels: 8, connectsPerIpPerMinute: 20, maxPushTokens: 16 } as const;
export const PushKind = z.enum(["approval", "limit", "routine_failed"]);
export const loginMessage = (hostId: string, nonce: string, relayHost: string) =>
  new TextEncoder().encode(`tjommie-relay-login/v1\n${hostId}\n${nonce}\n${relayHost}`);
// Control socket, text JSON, relay → host
export const RelayToHost = z.discriminatedUnion("t", [
  z.object({ t: z.literal("challenge"), nonce: RemoteKey }),                 // 32 random bytes, base64url
  z.object({ t: z.literal("ready"), push: z.boolean() }),
  z.object({ t: z.literal("open"), channel: RemoteKey }),                    // single use, expires after acceptTimeoutMs
  z.object({ t: z.literal("pushed"), id: z.number().int(), results: z.array(z.object({ ok: z.boolean(), unregistered: z.boolean() })).max(16) }),
]);
// Control socket, host → relay
export const HostToRelay = z.discriminatedUnion("t", [
  z.object({ t: z.literal("login"), publicKey: RemoteKey, signature: z.string().regex(/^[A-Za-z0-9_-]{86}$/) }),
  z.object({ t: z.literal("push"), id: z.number().int(), tokens: z.array(z.string().min(1).max(4096)).min(1).max(16), kind: PushKind, botId: z.string().max(64) }),
]);
export const PushData = z.object({ v: z.literal("1"), hostId: z.uuid(), kind: PushKind, botId: z.string().max(64) }).strict();
export const RelayClose = { hostOffline: 4404, badLogin: 4401, replaced: 4409, hostDead: 4408, tooLarge: 1009, limited: 4429 } as const;
```

### Relay HTTP surface (Worker → DO `idFromName(hostId)`; hostId must be a UUID, otherwise 400)

| Path (WebSocket upgrade) | Who | Behavior |
|---|---|---|
| `/v1/hosts/<hostId>/control` | host | challenge → login → ready. Newest valid login replaces the old one (4409). Text `"ping"` is auto-answered `"pong"` (`setWebSocketAutoResponse`, no DO wake). An alarm closes the host with 4408 after `hostDeadMs` without a ping. Push messages go to FCM. |
| `/v1/hosts/<hostId>/connect` | device | Host offline → close 4404. Otherwise send `open{channel}` to the host and wait ≤10 s for the host's channel leg (then 4404). Pair the legs, then forward each binary message 1:1 in both directions. One leg closing closes the other. |
| `/v1/hosts/<hostId>/channels/<channel>` | host | Accepts only a live unexpired channel id issued on the authenticated control socket. |
| `GET /v1/health` | anyone | `{ ok: true, relay: "tjommie-relay/1" }` |

Each binary WS message is exactly one Noise frame of 1..65,535 bytes, with no length prefix. Channel pairing state is stored in WS attachments (`serializeAttachment`) so it survives hibernation.

### Daemon seams (`packages/daemon/src/peer-wire.ts`, new)

```ts
export interface PeerWire {
  readonly kind: "lan" | "relay";
  readonly address: string;            // LAN: socket.remoteAddress; relay: "relay"
  readonly buffered: number;           // bytes waiting to be written
  write(frame: Uint8Array): void;      // one Noise frame
  destroy(): void;
  onFrame(listener: (frame: Uint8Array) => void): void;
  onClose(listener: () => void): void;
}
export function tcpWire(socket: Socket): PeerWire;           // length-prefixed via FrameDecoder/lengthPrefixed
export function webSocketWire(socket: WebSocket): PeerWire;  // one frame per binary message
// RemoteHost gains:
acceptWire(wire: PeerWire): void;
pushTargets(): { deviceId: string; token: string }[];        // non-revoked devices with a token
clearPushToken(deviceId: string): void;
ensureListening(): Promise<void>;                            // existing start()
```

### Client seams (`packages/client/src/frame-socket.ts`, new; `FrameTransport` moves here from `remote-peer.ts` and is re-exported there)

```ts
export type WireEndpoint = { kind: "lan"; host: string; port: number } | { kind: "relay"; url: string; hostId: string };
export interface FrameSocketApi { open(endpoint: WireEndpoint, timeoutMs: number): Promise<FrameTransport> }
export function relayConnectUrl(base: string, hostId: string): string;   // https→wss + /v1/hosts/<id>/connect
export function webSocketFrames(url: string, timeoutMs: number): Promise<FrameTransport>; // browser/Node WebSocket, binaryType arraybuffer
```

### UI connection states (`packages/ui/lib/state.ts`)

```ts
type Connection = /* existing */ | { status: "offline"; reason: "host_offline" | "unreachable" } | { status: "removed" };
// ConnectionTarget: { remoteHostId: string } replaces { mobileHostId: string } (Android and desktop).
export const LINK = { pingForegroundMs: 20_000, pingBackgroundMs: 60_000, pongTimeoutMs: 10_000, lanWindowMs: 1_000,
  backoffMs: [0, 1_000, 2_000, 5_000, 10_000] } as const;  // host closes peers idle > 150_000 ms
```

### Native (Android plugin `TjommieKeepAlive`, desktop preload `window.tjommie.remote`)

```ts
// packages/mobile/src/wrappers.ts
interface KeepAlivePlugin { start(o: { hostName: string }): Promise<void>; stop(): Promise<void>; status(): Promise<{ running: boolean }>;
  batteryUnrestricted(): Promise<{ value: boolean }>; openBatterySettings(): Promise<void> }
// TjommieSocket gains event "network" { type: "wifi" | "cellular" | "none" | "other" } from ConnectivityManager.NetworkCallback.
// packages/ui/lib/desktop.ts DesktopApi gains:
remote: { open(e: WireEndpoint, timeoutMs: number): Promise<{ id: string }>; send(id: string, frame: Uint8Array): Promise<void>; close(id: string): void;
  onFrame(l: (id: string, frame: Uint8Array) => void): () => void; onClosed(l: (id: string) => void): () => void;
  seal(name: string, value: Uint8Array): Promise<void>; read(name: string): Promise<Uint8Array | null>; delete(name: string): Promise<void> };
loginItem: { get(): Promise<boolean>; set(on: boolean): Promise<void> };
updates: { state(): Promise<UpdateState>; onState(l: (s: UpdateState) => void): () => void; check(): Promise<void>; restart(): void };
type UpdateState = { status: "idle" | "checking" | "none" } | { status: "downloading"; percent: number } | { status: "ready"; version: string } | { status: "error"; message: string } | { status: "unsupported" };
```

## Delivery order and waves

| Wave | Tasks (parallel within a wave) | Gate to the next wave |
|---|---|---|
| 0 | T0 Android background spike (needs the owner's phone OK), T1 contracts, T2 typecheck wiring | T1 merged; T0 verdict recorded in this plan |
| 1 | T3 relay, T4 host wire, T5 host relay client and push, T6 desktop always-on and update, T7 Android native (after T0), T9 UI connection core | Each task's tests are green and its cross-provider review is clear |
| 2 | T8 desktop remote bridge (after T6), T10 UI screens (after T9's state names land), T11 relay deploy constant (after the owner deploys) | Full suite plus no-any, typecheck and lint pass |
| 3 | T12 integration, packaging and acceptance; T13 security review (read-only, both providers) | Owner runs the demo |

Live agents never exceed 7. In wave 1, start T3, T4, T5, T6 and T9 first (5 agents) and add T7 when T0 reports. Reviewers take the free slots as tasks finish.

## Agent selection and file ownership

| Task | Agent | Owns (only this task edits these) | Reviewer |
|---|---|---|---|
| T0 | Codex gpt-6-sol max | throwaway spike branch, never merged | Opus 5.5 max |
| T1 | Codex gpt-6-sol max | `protocol/src/remote.ts`, `protocol/src/relay.ts`, `protocol/src/model.ts` (Settings), `protocol/src/index.ts`, `client/src/frame-socket.ts`, `client/src/remote-peer.ts` (type move), `daemon/src/peer-wire.ts` (types and `tcpWire` only) | Opus 5.5 max |
| T2 | gpt-6-luna max | root `package.json` scripts, root `tsconfig.json` | Opus 5.5 max |
| T3 | Codex gpt-6-sol max | `packages/relay/**`, `docs/relay-self-host.md`, root `package.json` workspaces entry | Opus 5.5 max |
| T4 | Codex gpt-6-sol max | `daemon/src/remote-host.ts`, `daemon/src/peer-wire.ts` (`webSocketWire`), `daemon/src/devices-store.ts`, `daemon/src/db.ts` (migration) | Opus 5.5 max |
| T5 | Codex gpt-6-sol max | new `daemon/src/relay-client.ts`, `relay-identity.ts`, `push.ts`; `daemon/src/daemon.ts`, `daemon/src/settings.ts` | Opus 5.5 max |
| T6 | Codex gpt-6-sol max | `desktop/src/main.ts`, `desktop/src/preload.cjs`, new `desktop/src/updater.ts`, `desktop/src/login-item.ts`, `desktop/scripts/package.ts`, `desktop/package.json` | Opus 5.5 max |
| T7 | Codex gpt-6-sol max | `packages/mobile/**` (native, wrappers, scripts, gradle, manifest, package.json) | Opus 5.5 max |
| T8 | Codex gpt-6-sol max | new `desktop/src/remote-wire.ts`, `desktop/src/device-keys.ts`; the `remote` block in `preload.cjs` and `main.ts` (after T6 merges) | Opus 5.5 max |
| T9 | Opus 5.5 max (+fast) | `ui/lib/connection.ts`, `ui/lib/mobile.ts` → `ui/lib/remote-hosts.ts`, `ui/lib/state.ts`, new `ui/lib/relay-wire.ts`, `ui/lib/push.ts`, `ui/lib/keep-alive.ts`, `ui/lib/desktop.ts` (types) | Codex gpt-6-sol max |
| T10 | Opus 5.5 max (+fast) | `ui/components/**`, `ui/app/settings/page.tsx` | Codex gpt-6-sol max |
| T11 | gpt-6-luna max | `DEFAULT_RELAY_URL` line in `protocol/src/relay.ts` | Opus 5.5 max |
| T12 | Codex gpt-6-sol max, plus the ops agent for releases | version files, `scripts/verify-packaged.ts`, real tests, `docs/superpowers/plans/2026-10-25-phase3-delivery-verification.md` | Opus 5.5 max |
| T13 | Opus 5.5 max **and** Codex gpt-6-sol max, read-only | none | n/a |

Dependency lock changes are serialized: T3, T6 and T7 each add dependencies, and the ops agent merges their `package-lock.json` changes one at a time.

---

## Tasks

Test commands run through the PowerShell tool from the worktree root (memory: npm/npx only via PowerShell; never `cd` in Bash). Every task ends with "hand the diff to the ops agent for commit". Implementers never commit.

### T0: Android background spike (wave 0, gate for T7)

**Question:** with an Android foreground service running, does the Capacitor WebView's JS (timers plus the `TjommieSocket` connection) stay alive on the S25 Ultra (Android 17, One UI)? Test with the screen locked for 30 min, under forced Doze, and through Samsung app sleeping.

- [ ] Build a throwaway APK with `applicationId dev.tjommie.app.spike`, so the installed app and its saved pairing are untouched. Add a minimal `remoteMessaging` foreground service (`FOREGROUND_SERVICE` + `FOREGROUND_SERVICE_REMOTE_MESSAGING`, started while visible, `stopWithTask`). Add a JS loop that sends an RPC ping to a **disposable** PC host profile every 60 s and records only counts and timestamps.
- [ ] With the owner's OK, run three passes on the S25:
  - screen off for 30 min;
  - `adb shell dumpsys deviceidle force-idle` for 15 min;
  - a Wi-Fi → mobile data switch.
  Record pings delivered against expected, and the battery drop per hour (`dumpsys batterystats --charged dev.tjommie.app.spike`).
- [ ] Repeat without the service as the baseline.
- [ ] Uninstall the spike APK and delete the disposable host profile.
- [ ] Record the verdict in this plan under "Integration evidence":
  - **PASS** means ≥95% of pings with the service. T7 and T9 then build "Always connected" as designed.
  - **FAIL** means T7 drops the service and T10 hides the toggle. The owner is told that background liveness falls back to reconnect-on-return plus FCM push.

### T1: Freeze contracts (wave 0)

**Files:** see the ownership table. Tests go in `protocol/src/remote.test.ts`, `protocol/src/relay.test.ts` and `client/src/frame-socket.test.ts`.

**Produces:** every name in "Frozen contracts" above, exactly.

- [ ] Write failing tests:
  - `PairingCode` accepts `v:2` with `relay: "https://r.example.workers.dev"` and with `null`.
  - It rejects `v:1`, `relay: "http://…"` and `relay: "wss://…"`.
  - `parsePairingCode` round-trips v2.
  - `RemoteReady` requires `relay`.
  - `RemoteRefused` accepts only `revoked|unknown`.
  - `HostToRelay` rejects 17 tokens and kind `"chat"`.
  - `PushData` rejects an extra `title` key.
  - `loginMessage("h","n","r.example")` encodes to the bytes of `"tjommie-relay-login/v1\nh\nn\nr.example"`.
  - `relayConnectUrl("https://r.example/", id)` gives `"wss://r.example/v1/hosts/<id>/connect"`.
  - `Settings` accepts `relayUrl: null`.
- [ ] Run `npx vitest run packages/protocol packages/client`. Expect FAIL.
- [ ] Implement the schemas.
  - Update `remote-host.ts:84` only enough to type-check: `v: 2, relay: null`. T4 owns the real change.
  - Update `ui/lib/mobile.ts` only for the import move. T9 owns the real change.
  - Add `relayUrl` to the daemon `DEFAULTS` as `DEFAULT_RELAY_URL`.
  - Update the old test fixtures (`remote.test.ts:5,23`, `mobile.test.ts:30`, `pairing.test.ts:8`) to v2.
- [ ] Run `npx vitest run packages/protocol packages/client packages/daemon/src/remote-host.test.ts` → PASS. Run `npm run typecheck` → PASS.

### T2: Typecheck wiring (wave 0, luna)

- [ ] Add the existing `typecheck` scripts of `@tjommie/transport` and `@tjommie/mobile` to the root `typecheck` chain (`check:no-any && tsc -p . && … -w @tjommie/ui && … -w @tjommie/transport && … -w @tjommie/mobile`). T3 appends `@tjommie/relay`.
- [ ] Run `npm run typecheck`. Expect PASS, or a list of pre-existing errors that the reviewer triages.

### T3: Relay Worker (wave 1)

**Files:**
- Create in `packages/relay/`: `package.json` (`@tjommie/relay`, scripts `dev`, `deploy`, `typecheck`), `wrangler.toml` (DO class `HostRoom` as `new_sqlite_classes`, `[observability] enabled = false`), `src/worker.ts`, `src/room.ts`, `src/fcm.ts`, `tsconfig.json`.
- Docs: `docs/relay-self-host.md`.
- Tests: `src/room.test.ts`, `src/fcm.test.ts`, `src/relay.real.test.ts`.

**Interfaces:**
- Consumes: T1 `relay.ts`.
- Produces: the HTTP surface table above.

**Design:**
- `room.ts` is pure logic: `class HostRoomCore` with injected `Sock` (`send`, `close`, `attachment get/set`), `clock` and `random`.
- `worker.ts` is the thin glue to `DurableObject`, `acceptWebSocket`, `webSocketMessage`, `webSocketClose`, `alarm` and `setWebSocketAutoResponse`.
- `fcm.ts` holds `sendPush(env, tokens, data): Promise<{ok, unregistered}[]>`:
  - `jose` `SignJWT` signs RS256 with the service account, exchanged at `https://oauth2.googleapis.com/token` with scope `firebase.messaging`. The access token is cached until `exp − 60 s`.
  - It POSTs `projects/<id>/messages:send` with:
    - `notification {title: "Tjommie", body}` and `data: PushData`;
    - `android {priority: "high", notification {tag: kind + ":" + botId, channel_id: "bots"}}`.
  - Bodies: approval → "A bot needs you", limit → "An account hit its limit", routine_failed → "A routine failed".
  - `FCM_ENDPOINT` and `FCM_TOKEN_ENDPOINT` env overrides exist for tests.
  - HTTP 404 or `UNREGISTERED` → `unregistered: true`.
  - With no `FCM_SERVICE_ACCOUNT`, `ready.push = false` and push messages answer `ok:false`.

- [ ] Write failing `room.test.ts` cases with a fake clock and fake socks:
  - login with a valid signature → `ready`; first login stores the key;
  - a later login with a different key → 4401 and the first host stays online;
  - a second valid login → the old one is closed 4409;
  - device connect with the host offline → 4404;
  - device connect → host gets `open`, then the channel leg pairs and binary frames forward in both directions unchanged;
  - accept after 10 s → 4404;
  - a 65,536-byte message → 1009;
  - the 21st connect per IP in a minute → 4429;
  - a 33rd channel → 4429;
  - host dead after 75 s without a ping → 4408 and its channels are closed;
  - a push with 17 tokens is rejected;
  - no `console` call contains a token or IP (spy on console).
- [ ] Write failing `fcm.test.ts`: JWT claims (`iss`, `scope`, `aud`, `exp − iat = 3600`); payload shape equals the spec above; `UNREGISTERED` maps to `unregistered: true`; the token is cached across two sends.
- [ ] Run `npx vitest run packages/relay`. Expect FAIL.
- [ ] Implement `room.ts`, `fcm.ts` and `worker.ts`.
- [ ] Write `relay.real.test.ts`:
  - It spawns `npx wrangler dev --local --port 0` with `windowsHide: true`.
  - A Node host logs in with an `ed25519` key, a Node device connects, and frames round-trip.
  - Killing the host's socket gives 4404 on the next connect.
- [ ] Run `npx vitest run packages/relay` → PASS. Run `npx vitest run --mode real packages/relay/src/relay.real.test.ts` → PASS.
- [ ] Write `docs/relay-self-host.md`:
  - prerequisites;
  - `wrangler login`, then `npm run deploy -w @tjommie/relay`;
  - the optional `FCM_SERVICE_ACCOUNT` secret, and the fact that push needs a matching APK build;
  - where to paste the URL (Settings → Devices → Relay address);
  - what the relay can see (invariant 1).
- [ ] Append `@tjommie/relay` typecheck to the root chain.

### T4: Host accepts any wire, heartbeat, refusal, pairing v2, push tokens (wave 1)

**Files:**
- Modify: `daemon/src/remote-host.ts`, `daemon/src/peer-wire.ts`, `daemon/src/devices-store.ts`.
- Modify `daemon/src/db.ts`: next migration `ALTER TABLE devices ADD COLUMN push_token TEXT;`.
- Tests: `remote-host.test.ts`, `peer-wire.test.ts`, `devices-store.test.ts`, `db.test.ts`.

**Interfaces:**
- Consumes: T1 types.
- Produces: `RemoteHost.acceptWire`, `pushTargets`, `clearPushToken`, and constructor option `relayUrl: () => string | null`. T5 passes it.

- [ ] Write failing tests:
  - **Same behaviour over both wires.** Run the existing `remote-host.test.ts` pairing and resume suite through a `fakeWire()` with `kind: "relay"`, as well as over TCP.
  - **Refusals.**
    - `"refused after revoke"`: a revoked key on resume completes the handshake, then receives `RemoteRefused{reason:"revoked"}` and is closed.
    - An unknown key gets `reason:"unknown"`.
  - **Pairing v2 and ready.** The code is `v:2`, and `relay` equals `relayUrl()`. `RemoteReady.relay` equals `relayUrl()`.
  - **Ping and idle.** `remote.ping` returns `{}`. A peer with no inbound frame for 150 s is closed by the 30 s sweep. Use fake timers.
  - **Push tokens.**
    - `device.registerPush` over a remote connection sets that device's `push_token` and never reaches `dispatch`.
    - `{token:null}` clears it.
    - Revoke clears `push_token`.
    - `pushTargets()` excludes revoked devices.
  - **Relay admission.** Relay wires skip the per-address counter but still count toward the 32 active / 8 pending limits.
  - **Migration.** It applies on the previous schema version.
- [ ] Run `npx vitest run packages/daemon/src/remote-host.test.ts packages/daemon/src/db.test.ts`. Expect FAIL.
- [ ] Implement:
  - Replace `#accept(socket)` with `acceptWire(wire)`. The TCP server calls `acceptWire(tcpWire(socket))`.
  - Track `lastFrameAt` per peer.
  - Send `RemoteRefused` instead of throwing in the `handshake` phase when the key is missing or revoked.
- [ ] Run the tests → PASS.

### T5: Host relay client, relay identity, push sender, settings (wave 1)

**Files:**
- Create: `daemon/src/relay-identity.ts`, `daemon/src/relay-client.ts`, `daemon/src/push.ts`.
- Modify: `daemon/src/daemon.ts` (wiring, broadcast hook, handlers `remote.ping → {}`, local `device.registerPush → RpcError("bad_state")`), `daemon/src/settings.ts`.
- Tests: matching `*.test.ts` files and `relay-client.real.test.ts` (against T3's `wrangler dev`).

**Interfaces:**
- Consumes:
  - T4: `acceptWire`, `pushTargets`, `clearPushToken`, `relayUrl` option.
  - T1: `relay.ts`.
  - The existing `windowsCredentialTransport`.
- Produces:
  - `loadRelayIdentity({ root, credentials }): Promise<{ publicKey: Uint8Array; sign(message: Uint8Array): Uint8Array }>`: Ed25519 via `node:crypto`, stored at target `Tjommie/<sameHash>/relay-login`.
  - `class RelayClient { constructor(o: { url: () => string | null; hostId: string; identity; onChannel: (wire: PeerWire) => void; onState: () => void }); start(): void; stop(): void; state(): RelayState; push(tokens: string[], kind, botId): Promise<{ok, unregistered}[]> }`.
  - `startPush({ host: RemoteHost, relay: RelayClient }): (notify: EventData<"notify">) => void`.

**Behavior:**
- **When it connects.** It connects only while `remote.hasDevices()` or a pairing invite is active, and `relayUrl` is non-null.
- **Reconnecting.**
  - Backoff `[1, 2, 5, 10, 30] s`, capped at 30 s.
  - Ping `"ping"` every 30 s.
  - A 5 s interval timer that sees more than a 15 s gap (sleep or wake) reconnects at once.
  - On `open{channel}`, it dials `/channels/<channel>` and passes `webSocketWire(ws)` to `onChannel`.
- **Settings.** `settings.set {relayUrl}` restarts the client. Existing LAN peers are untouched.
- **Push.**
  - On every `notify` broadcast, push to all `pushTargets()` (at most 16 tokens per message).
  - At most 1 push per (device, kind, botId) per 30 s, and 30 per device per hour.
  - `unregistered` → `clearPushToken`.
  - Push failures are counted, never logged with tokens.
- **Status.** `remote.status.relay = { url, state, push }`.

- [ ] Write failing tests with a fake WebSocket server:
  - the login signature verifies against `loginMessage`;
  - `"drift triggers relogin"`: a fake clock jump of 60 s reconnects within 5 s;
  - backoff sequence;
  - `open` → `onChannel` called with a relay wire;
  - a `relayUrl` change restarts;
  - push coalescing and rate limit;
  - unregistered clears the token;
  - a log spy finds no token, key or channel id;
  - the identity round-trips through a synthetic credential transport, and a mismatched vault throws without minting.
- [ ] Run `npx vitest run packages/daemon/src/relay-client.test.ts packages/daemon/src/push.test.ts packages/daemon/src/relay-identity.test.ts`. Expect FAIL.
- [ ] Implement them and wire them into `daemon.ts`: the broadcast at `:502` also feeds `startPush`, and `pairing.create` starts the relay.
- [ ] Run the tests → PASS. Unit tests fake `webSocketWire`. Once T4 and T3 have merged, run `relay-client.real.test.ts`: a real daemon `RemoteHost` and a Node device client pair through `wrangler dev` with no LAN endpoint reachable. Expect PASS.

### T6: Desktop always-on and auto-update (wave 1)

**Files:**
- Create: `desktop/src/login-item.ts`, `desktop/src/updater.ts`, plus tests for both.
- Modify: `desktop/src/main.ts`, `desktop/src/preload.cjs`, `desktop/scripts/package.ts`, `desktop/package.json` (add `electron-updater`).
- Create: `desktop/src/updater.real.test.ts` (packaged mode).

**Interfaces:**
- Produces the preload `loginItem` and `updates` blocks, and the `UpdateState` type from the frozen contracts.

**Behavior:**
- **Login item.** `syncLoginItem(app)`: at the first packaged run (marker `userData/login-item-set`), call `setLoginItemSettings({ openAtLogin: true, args: ["--hidden"] })`. `get` and `set` read and write it with the same args.
- **Hidden start.** With `--hidden` in `process.argv`, `ready-to-show` does not show the window. A second instance, the tray or a notification click shows it.
- **Tray menu.**
  - "Open Tjommie", separator.
  - "Restart to update to v<version>" (only when the update is `ready`).
  - "Quit Tjommie".
- **Updater.**
  - Skip when `!app.isPackaged` or `TJOMMIE_UPDATES=off` (state `unsupported`).
  - `autoDownload: true`, `autoInstallOnAppQuit: true`, `logger: null`.
  - Check 60 s after start and every 6 h.
  - `restart()` → `autoUpdater.quitAndInstall(true, true)`, through the existing `will-quit` cleanup so the daemon stops first.
- **package.ts.**
  - `publish: [{ provider: "github", owner: "tsnyders", repo: "tjommie" }]`, built with `--publish never`. The ops agent uploads `Tjommie-Setup-<v>.exe`, `.blockmap` and `latest.yml`.
  - `artifactName: "Tjommie-Setup-${version}.${ext}"`.
  - No `publisherName`, because the build is unsigned.
  - Env `TJOMMIE_PACKAGE_APP_ID`, `TJOMMIE_PACKAGE_PRODUCT` and `TJOMMIE_UPDATE_FEED` (generic provider URL) exist for the update test only.

- [ ] Write failing unit tests:
  - `--hidden` → window not shown;
  - first run sets the login item once, and the marker prevents a second set;
  - the tray menu has the restart item only when `ready`;
  - updater state transitions from fake `autoUpdater` events;
  - `unsupported` when unpackaged.
- [ ] Run `npx vitest run packages/desktop/src/login-item.test.ts packages/desktop/src/updater.test.ts`. Expect FAIL.
- [ ] Implement.
- [ ] Write `updater.real.test.ts`:
  - Package 0.3.0-test and 0.3.1-test with appId `com.tsnyders.tjommie.updatetest` and productName "Tjommie Update Test". Use the isolated data root that `packaged.real.test.ts` already uses.
  - Serve 0.3.1's `latest.yml` and installer from a local HTTP server.
  - Install 0.3.0-test silently, launch it with `TJOMMIE_UPDATE_FEED`, and wait for `ready`. Call `restart`, then assert:
    - the daemon child exited;
    - the installed version is 0.3.1;
    - the app relaunched hidden;
    - no new console window (no `conhost` children with a visible window).
  - Uninstall with `/S`. The owner's install is never touched.
- [ ] Run `npx vitest run packages/desktop` → PASS. Run the update test → PASS.

### T7: Android native: keep-alive, network events, push, release signing (wave 1, after T0)

**Files:**
- `packages/mobile/native/java/dev/tjommie/app/`: new `TjommieKeepAlivePlugin.java`, `KeepAliveService.java`, `TjommiePushRouting.java` (tap → JS); modify `TjommieSocketPlugin.java` (network event), `MainActivity.java`.
- Also: `native/AndroidManifest` changes applied by `scripts/apply-native.mjs`, `android/app/build.gradle`, `package.json` (add `@capacitor/push-notifications` only; network changes come from the `TjommieSocket` `network` event, so no `@capacitor/network`), `src/wrappers.ts`, `src/plugins.ts`.
- Tests: JVM tests in `native/java-test/`, and `src/wrappers.test.ts`.

**Interfaces:**
- Produces: `KeepAlivePlugin` and the `TjommieSocket` `network` event from the frozen contracts.
- Push uses the official plugin's `register`, `registration`, `pushNotificationActionPerformed` and `checkPermissions`/`requestPermissions`.

**Behavior:**
- **Keep-alive service.**
  - `remoteMessaging` foreground service, started only from a visible activity.
  - Notification channel "Connection" (low importance): title "Connected to <PC>", tap opens the app.
  - `stopWithTask="true"`.
  - `batteryUnrestricted` uses `PowerManager.isIgnoringBatteryOptimizations`. `openBatterySettings` fires `ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS`, falling back to app details.
- **Network events.** Emit `network` on `onAvailable`, `onLost` and `onCapabilitiesChanged` when the transport type changes.
- **Push.**
  - Notification channel "bots" (high importance), with `POST_NOTIFICATIONS` declared.
  - `apply-native.mjs` copies `$TJOMMIE_GOOGLE_SERVICES_JSON` into `android/app/` when it is set. Add `google-services.json` to `.gitignore`.
  - Without the file, the build sets a `BuildConfig.PUSH = false` flag that the wrapper exposes as `push.available() → false`.
- **Release signing.**
  - `signingConfigs.release` comes from the four `TJOMMIE_ANDROID_*` env vars.
  - `npm run package:android -- --release` refuses without them and writes `dist-mobile/Tjommie-Android-<v>.apk` plus provenance.
  - `versionCode` comes from package.json: 0.3.0 → 3.
  - Test with a throwaway keystore made by `keytool` in a temp dir.
- [ ] Write failing JVM tests:
  - network type mapping;
  - keep-alive notification text and channel ids;
  - the release signing gradle task fails without env.
- [ ] Write failing TS wrapper tests for `push.available()` and keep-alive status.
- [ ] Run `npm run native:apply -w @tjommie/mobile`, then `packages/mobile/android/gradlew.bat -p packages/mobile/android testDebugUnitTest` (the same JVM suite the 2026-10-22 delivery ran). Also run `npx vitest run packages/mobile`. Expect FAIL.
- [ ] Implement.
- [ ] Run them → PASS. Build both the debug APK and the throwaway-key release APK. Then `apksigner verify` → PASS.

### T8: Desktop remote bridge for "Connect to another computer" (wave 2, after T6)

**Files:**
- Create: `desktop/src/remote-wire.ts`, `desktop/src/device-keys.ts`, plus tests.
- Modify: `desktop/src/main.ts` (IPC handlers), `desktop/src/preload.cjs` (`remote` block).

**Behavior:**
- **Wires.** `open({kind:"lan"})` uses `net.Socket` with `lengthPrefixed`/`FrameDecoder` from `client/src/encrypted-channel.ts`. `open({kind:"relay"})` uses `webSocketFrames`. The limits mirror `TjommieSocketPlugin`: 8 connections, 32 MiB backlog, 65,535-byte frames.
- **Cleanup.** Wires close when their renderer reloads or is destroyed.
- **Keys.** `seal`, `read` and `delete` go through `windowsCredentialTransport` at `Tjommie/device/<name>`, with names matching `[a-z0-9._-]{1,64}`.
- **CSP.** It stays `ws://127.0.0.1:*`. All remote traffic goes through main.
- [ ] Write failing tests:
  - LAN round-trip against a local `net` server;
  - an oversized frame closes the wire;
  - the 9th open is rejected;
  - a reload closes wires;
  - key seal and read round-trip with a synthetic credential transport.
- [ ] Run `npx vitest run packages/desktop/src/remote-wire.test.ts packages/desktop/src/device-keys.test.ts`. Expect FAIL.
- [ ] Implement.
- [ ] Run them → PASS.

### T9: UI connection core: never-drop link, relay leg, push registration (wave 1)

**Files:**
- Modify: `ui/lib/connection.ts`, `ui/lib/state.ts`, `ui/lib/desktop.ts` (types).
- Rename `ui/lib/mobile.ts` → `ui/lib/remote-hosts.ts`, keeping a re-export of the old names until T10 switches imports.
- Create: `ui/lib/relay-wire.ts`, `ui/lib/push.ts`, `ui/lib/keep-alive.ts`.
- Tests: `connection.resume.test.ts`, `connection.heartbeat.test.ts`, `remote-hosts.test.ts`, `state.test.ts`, `push.test.ts`.

**Interfaces:**
- Consumes:
  - T1: `WireEndpoint`, `FrameSocketApi`, `relayConnectUrl`, `RemoteRefused`, `LINK`.
  - T7 and T8: native shapes, faked in tests.
- Produces:
  - Connection states `offline` and `removed`, and the target `{remoteHostId}`.
  - Platform `frameSockets(): FrameSocketApi`: Android LAN → `TjommieSocket`, Android relay → `webSocketFrames`, desktop → `window.tjommie.remote`.
  - `remoteHosts()`, `connectRemote(id)`, `pairRemote(code, name)` and `forgetRemote(id)`, which are the renamed `mobile*` functions, working on both platforms.
  - Saved host schema adds `relay: RelayUrl.nullable().default(null)`, updated from `RemoteReady.relay`.
  - `push.ts`: `registerPushIfAllowed(client)`, `onPushTap(handler)`.
  - `keep-alive.ts`: `syncKeepAlive(status)`.

**Behavior:**
- **Endpoint order.** Open all LAN endpoints at once with a 1 s window. The first to finish the Noise handshake wins and the rest close. Then try the relay. The 30 s deadline stays.
- **Heartbeat.** `remote.ping` every 20 s while visible (60 s while hidden with keep-alive running). No reply within 10 s → close and reconnect at once.
- **Reconnect triggers.** `visibilitychange → visible`, the native `network` event, and a heartbeat failure all reconnect immediately (backoff index 0). Backoff runs `[0, 1, 2, 5, 10] s`, repeating 10 s while visible. While hidden without keep-alive, retries stop, and the next visible event starts again.
- **No full-screen failure for a saved host.**
  - With a snapshot already loaded, a failure keeps the cached UI and sets `offline`. The reason is `host_offline` for relay close 4404, otherwise `unreachable`.
  - `failed` is now only for the desktop's local daemon, and for a first connect with no snapshot.
  - `RemoteRefused` → `removed`, which is terminal and stops retries.
- **Catch-up without blanking.** `connected()` keeps loaded conversations. Each one refetches its newest page and merges:
  - if the fetched page's oldest id is already loaded, replace from that id onward;
  - otherwise replace the conversation with the page, with `hasMore: true`.
  The snapshot lists (bots, rooms, routines, accounts, settings) refresh as today, plus `notifications.pending`.
- **Push.** After `welcome` on a remote connection, call `device.registerPush {token}` if permission is granted and `push.available()`. On a tap, connect to `data.hostId` if it isn't current, then route to `/chat/?bot=<botId>`. While the app is in front, a received push is ignored.
- **Keep-alive.** Start it while the remote connection is `ready` and the setting `tjommie.mobile.keepAlive.v1` (localStorage, default `true`) is on. Stop it on `removed`, forget or toggle off.

- [ ] Write failing `connection.resume.test.ts` with a fake `FrameSocketApi` and fake timers:
  - "locked 30 min then visible": the old wire is silently dead, and `visibilitychange` reconnects at once. The state goes `ready → reconnecting → ready` without `failed`. A message added on the fake host while disconnected appears in the open chat, and older loaded messages stay.
  - "host offline over relay": relay close 4404 → `{status:"offline", reason:"host_offline"}` while bots stay listed.
  - "revoked": `RemoteRefused{revoked}` → `removed`, and no further `open` calls after 60 s of fake time.
- [ ] Write failing `connection.heartbeat.test.ts`:
  - no ping reply in 10 s → a new wire opens;
  - a `network` event → a new wire opens immediately;
  - an in-flight `chat.send` rejects on loss and is not resent;
  - a LAN race where the second endpoint wins closes the first;
  - with every LAN endpoint failing, the relay is tried after 1 s.
- [ ] Write failing `push.test.ts` (register after welcome only when allowed, tap routing across hosts) and `state.test.ts` (merge rule).
- [ ] Run `npx vitest run packages/ui/lib`. Expect FAIL.
- [ ] Implement.
- [ ] Run → PASS. Run `npm run typecheck -w @tjommie/ui` → PASS.

### T10: UI screens (wave 2)

**Files:**
- `ui/components/devices/devices-section.tsx`, `pair-dialog.tsx`, `mobile-connect.tsx` (it becomes the shared "Connect to a PC" screen), `connection-screen.tsx`, `app-shell.tsx`, `rail.tsx`.
- New `ui/components/host-switcher.tsx` and `ui/components/devices/relay-row.tsx`.
- `ui/app/settings/page.tsx` (General).
- Tests: alongside each component; update `devices.test.tsx:14-20`, which currently asserts the absence of "relay" and "push".

**Consumes:** T9 states and functions, T6 `loginItem` and `updates`, T5 `remote.status.relay`.

**Screens and copy:**
- **Status pill** (`app-shell.tsx`), over the cached UI:
  - "Reconnecting…" (`reconnecting`);
  - "Your PC is offline" (`offline/host_offline`);
  - "Can't reach your PC" (`offline/unreachable`).
  The pill never blocks the screen.
- **Removed screen:** "This PC removed this device" with "Pair again" and "Forget".
- **Settings → Devices (host side):**
  - Copy: "Phones and other PCs can connect over Wi-Fi or, through the relay, from anywhere."
  - A **Relay** row: the address field (https), state "Connected / Connecting / Can't reach the relay / Off", "Push: available / not set up on this relay", and "Use the public relay" (resets to `DEFAULT_RELAY_URL`, hidden when that is null).
  - The Windows Firewall note stays.
- **Settings → Devices (phone side):**
  - "Always connected" switch (default on), with the note "Keeps a notification showing so Tjommie stays connected in the background. Uses a little more battery."
  - When battery isn't unrestricted, a row "Let Tjommie run in the background" opens battery settings, with the Samsung hint "On Samsung: Settings → Battery → Background usage limits → Never sleeping apps → add Tjommie."
  - A "Notifications" row: Allow, On, Off, or "Not in this build".
- **Pair dialog:** the copy says pairing works on the same Wi-Fi or over mobile data when the relay is on.
- **Host switcher.**
  - Desktop: the rail's bottom avatar (`rail.tsx:48-50`) becomes a menu button: "This PC", then saved remote PCs (each with a status dot), then "Connect to another computer…".
  - That last item opens the shared connect screen without a scanner. The device name defaults to "Another PC".
  - Phone: the existing "Switch PC" pill opens the same list.
  - While on a remote host, the desktop shows the host name in the menu button, and the native browser card shows "Runs on <PC>".
- **Settings → General (desktop only):**
  - "Start Tjommie when you sign in to Windows" switch.
  - An "Updates" row: "Up to date / Checking / Downloading 42% / Ready: Restart to update to v0.3.1 / Couldn't check: <message>", and "Check now".
- [ ] Write failing component tests for each bullet's visible text and actions, with fake state:
  - the pill states and the removed screen;
  - the relay row save calls `settings.set {relayUrl}`;
  - the Always connected toggle calls `syncKeepAlive`;
  - the host switcher lists hosts and calls `connectRemote`;
  - the update row restart calls `updates.restart`;
  - the General login toggle calls `loginItem.set`.
- [ ] Run `npx vitest run packages/ui/components`. Expect FAIL.
- [ ] Implement.
- [ ] Run → PASS. Run `npm run lint` → PASS. Run `phone-layout.real.test.ts` and `screens.real.test.ts` with the synthetic profile (memory: phone-layout overwrites tracked screenshots, so review the diffs).

### T11: Public relay constant (wave 2, after the owner deploys; luna)

- [ ] Set `DEFAULT_RELAY_URL` in `protocol/src/relay.ts` to the URL the owner reports. Add a test asserting it parses with `RelayUrl`. Run `npx vitest run packages/protocol` → PASS.

### T12: Integration, packaging and delivery record (wave 3)

- [ ] Bump the root and workspace versions to `0.3.0`, and the APK `versionCode` to 3.
- [ ] Run the full gate: `npm run typecheck`, `npm run lint`, `npm test`, `npx vitest run --mode real packages/relay packages/daemon/src/relay-client.real.test.ts` and `npm run test:packaged`.
- [ ] Run a new `packages/desktop/src/two-hosts.real.test.ts`:
  - Two isolated daemon profiles on this PC (A as host, B as a desktop client) plus a local `wrangler dev` relay.
  - B pairs with A's pasted code with LAN blocked (A's code lists only an unroutable LAN address), so pairing goes through the relay.
  - B chats with a paused synthetic bot, sees "Runs on <PC>" for the browser, and switches back to "This PC".
  - Revoking B on A → B shows the removed screen within 2 s.
- [ ] `npm run package` (unsigned) and `npm run package:android` (debug, or release when the owner supplies the keystore env). Record artifacts, sizes, SHA-256 and provenance in `docs/superpowers/plans/2026-10-25-phase3-delivery-verification.md`, in the format of `2026-10-22-android-delivery-verification.md`.
- [ ] With the owner's OK, run S25 acceptance with the **disposable** PC profile, as in the 2026-10-22 delivery. Cover:
  - pairing over mobile data (Wi-Fi off on the phone) by pasted code, plus a QR scan if the owner is present;
  - the locked 30 min, background, network switch and reopen check (Verification step 3);
  - revoke.
- [ ] Hand to the ops agent: the branch, PR, and a GitHub Release draft with `Tjommie-Setup-0.3.0.exe`, `.blockmap`, `latest.yml` and the APK. The release is published only after the owner approves.

### T13: Security review (wave 3, read-only, both providers)

The scope is invariants 1–7 against the merged branch:
- the relay's stored and logged data;
- the login signature domain separation;
- channel id single use;
- the refusal path's information leak;
- push payload content;
- token storage, clearing and logging;
- updater integrity without signing;
- the desktop bridge limits;
- the CSP being unchanged.

Findings go back to the owning task's agent. No reviewer edits files.

---

## Verification

**Automated (agents):**
1. **Gates:** `npm run typecheck` (now including transport, mobile and relay), `npm run check:no-any`, `npm run lint`, `npm test`. Known flakes: native-browser unit tests under load. Rerun bounded and report both runs.
2. **Relay:** `room.test.ts`, `fcm.test.ts` and `relay.real.test.ts` on miniflare. No Cloudflare account is used.
3. **Host:** both-wire `remote-host.test.ts`, `relay-client.real.test.ts` and push rate limits. No secrets in the log spies.
4. **UI:** `connection.resume.test.ts` and `connection.heartbeat.test.ts` (Review Focus 1–2), component tests, phone layout and screens real tests.
5. **Desktop:** `updater.real.test.ts` with the test appId, `two-hosts.real.test.ts`, and `npm run test:packaged`.
6. **Android:** the JVM suite, `apksigner verify`, and the T0 spike verdict.

**Owner real-device run (the owner does these with real accounts and devices):**
1. **Deploy and configure.** Deploy the relay and set the FCM secret. Install `Tjommie-Setup-0.3.0.exe`. Uninstall the debug APK on the S25 and install the release APK.
2. **Sign out of Windows and back in.** Tjommie is in the tray with no window. Closing the window later keeps the tray. Tray → Quit ends both `Tjommie.exe` and the daemon (Task Manager).
3. **Phone liveness.**
   - With the S25 on mobile data (Wi-Fi off), pair by QR. Allow notifications and keep "Always connected" on.
   - Lock the phone for 30 min, switch Wi-Fi on and off once, then unlock and open Tjommie. It is live immediately with no reconnect screen, and a message a bot sent meanwhile is there.
   - Turn "Always connected" off, swipe the app away, wait 10 min, then reopen. It's live within about a second.
4. **On the phone over mobile data:** chat with a bot, approve a tool request, and create a bot.
5. **Push.** Close the app (swipe away). Make a bot ask for approval on the PC. A "A bot needs you" push arrives, with no bot text. Tapping it opens that bot's chat.
6. **Second PC.** On the second PC, install Tjommie, then rail menu → "Connect to another computer…" and paste the code. Chat from it, then switch back to "This PC".
7. **Revoke.** Revoke the phone on the host. Within 2 s the phone shows "This PC removed this device", and it never reconnects.
8. **Sleep.** Put the PC to sleep. The phone shows "Your PC is offline" within about 75 s. Wake the PC, and the phone reconnects by itself.
9. **Update.** Publish 0.3.1 (ops agent, owner-approved). Within 6 h, or via Settings → General → Check now, "Restart to update" appears. Restarting installs silently, with no console window, and Tjommie comes back in the tray.

## Acceptance mapped to the Phase 3 demo

| Demo line | Proven by |
|---|---|
| The app starts at login | T6 login-item tests; owner step 2 |
| Pair an Android phone by QR over mobile data | T3 relay plus T4/T5 relay pairing (`relay-client.real.test.ts`), T9 LAN-then-relay race; owner step 3 |
| Chat, approve and create a bot from the phone | Existing RPC parity over the new relay wire (T4 both-wire suite); owner step 4 |
| A push arrives when a bot waits | T3 `fcm.test.ts`, T5 push tests, T7/T9 registration and tap; owner step 5 |
| Pair a second PC with the code | T8 bridge, T10 host switcher, T12 `two-hosts.real.test.ts`; owner step 6 |
| Revoke the phone and it's cut off at once | T4 "refused after revoke", T9 `removed`, T12 two-hosts revoke; owner step 7 |
| **Added (owner, 2026-10-25):** the phone stays connected, with no manual reconnect while the PC is up | T0 spike, T9 resume and heartbeat tests, T7 keep-alive; owner step 3 |
| Host offline when the PC sleeps; auto-update (spec §9.2.1) | T3 host-dead test, T5 drift relogin, T6 updater real test; owner steps 8–9 |

## Out of scope

- Business-team features: per-bot connectors, send/post approval, and Discord/webhook triggers. They get a later plan and may reuse `push.ts` and the relay control socket.
- Phase 4: live computer panel and vault.
- Play Store publishing.
- An in-app "new APK available" check.
- Desktop toasts for a remote host's notifications.
- iOS.
- Signed Windows builds: add `publisherName` and a signing config when a certificate exists.

## Integration evidence

(Filled in during execution: T0 verdict, gate results, artifact hashes.)
