# Self-host the Tjommie relay

The relay is a Cloudflare Worker with one SQLite-backed Durable Object for each host. You need a Cloudflare account, Node.js 24 or newer, and this repository's dependencies installed with `npm ci`.

Workers Free allows 100,000 Durable Object requests and 100,000 rows written per day across the account; each scheduled alarm writes a row. Check [Cloudflare's current pricing](https://developers.cloudflare.com/durable-objects/platform/pricing/) before choosing a plan for an always-on relay.

From the repository root, sign in and deploy:

```sh
npx wrangler login
npm run deploy -w @tjommie/relay
```

Copy the HTTPS `workers.dev` URL from Wrangler into **Settings → Devices → Relay address** on the host. Pair devices using a new pairing code from that host. A host ID is registered to its first valid Ed25519 login key; keep the host's credential store when moving the host to another PC.

Push is optional. To enable it, create a Firebase service-account key for your own Firebase project and set `FCM_SERVICE_ACCOUNT` as a Wrangler secret. The value is the complete service-account JSON. Push requires an APK built for that same Firebase project; another project's APK cannot receive these messages. The relay sends fixed notification text and only the push kind, host ID and bot ID as data.

```sh
npx wrangler secret put FCM_SERVICE_ACCOUNT
```

The relay can see connection IP addresses, timing, frame sizes, host IDs and channel IDs. For push it also sees the device token and notification kind. Its SQLite database stores only the host's login public key; socket attachments hold temporary channel pairing metadata. Noise frames pass through unchanged; the relay does not decrypt them, and it does not store frames, push tokens or IP addresses. Worker observability is disabled in `wrangler.toml`.
