import { existsSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { expect, it } from "vitest";
import { exportDesktopUi } from "./desktop-ui-export.ts";
import { tempRoot } from "./testing.ts";

const THEME = "tjommie.theme", TEXT = "tjommie.textSize", DENSITY = "tjommie.density", HOSTS = "tjommie.mobile.hosts.v1", PENDING = "tjommie.mobile.pending.v1";
type Stored = Record<string, string>;

// What the page's localStorage would answer; a key it doesn't hold is null, like the real thing.
const page = (stored: Stored) => async (name: string): Promise<string | null> => stored[name] ?? null;
const publicKey = (byte: number): string => Buffer.alloc(32, byte).toString("base64url");
const host = (n: number, changes: Record<string, unknown> = {}): Record<string, unknown> => ({
  id: `5b0e2c8e-6f1a-4c55-9a43-2f0d7b1e9c${n.toString(16).padStart(2, "0")}`,
  name: `Synthetic PC ${n}`,
  deviceId: null,
  hostKey: publicKey(n),
  lan: [{ host: "192.168.1.20", port: 47101 }],
  relay: null,
  ...changes,
});
const exportFile = (root: string): string => join(root, "data", "desktop-ui-export.json");
const exported = (root: string): unknown => JSON.parse(readFileSync(exportFile(root), "utf8"));
const lines = (): { log: (line: string) => void; seen: string[] } => {
  const seen: string[] = [];
  return { log: (line) => { seen.push(line); }, seen };
};

it("writes all five fields", async () => {
  const root = tempRoot();
  const first = host(1, { deviceId: "8d1f4c1e-2b7a-4e0b-b1d3-0a6c5e9f7a22", relay: "https://relay.example.workers.dev" });
  const second = host(2, { lan: [{ host: "fe80::1", port: 47102 }, { host: "10.0.0.7", port: 47101 }] });
  const waiting = host(3);
  await exportDesktopUi(page({ [THEME]: "light", [TEXT]: "large", [DENSITY]: "compact", [HOSTS]: JSON.stringify([first, second]), [PENDING]: JSON.stringify([waiting]) }), root);
  expect(exported(root)).toEqual({ version: 1, theme: "light", textSize: "large", density: "compact", hosts: [first, second], pending: [waiting] });
});

it("drops an invalid host and keeps the rest", async () => {
  const root = tempRoot();
  const { log, seen } = lines();
  const good = host(1), alsoGood = host(3);
  const badKey = host(2, { hostKey: "not-a-key" }), extraField = host(4, { lastSeen: "yesterday" });
  await exportDesktopUi(page({ [HOSTS]: JSON.stringify([good, badKey, alsoGood, extraField, "nonsense"]) }), root, log);
  expect(exported(root)).toMatchObject({ hosts: [good, alsoGood] });
  expect(seen.filter((line) => line.includes(HOSTS))).toHaveLength(3);
  // The log says which entry failed, never what was in it.
  expect(seen.join("\n")).not.toContain(badKey.hostKey);
});

it("keeps at most 16 hosts", async () => {
  const root = tempRoot();
  const { log, seen } = lines();
  const many = Array.from({ length: 17 }, (_unused, index) => host(index + 1));
  await exportDesktopUi(page({ [HOSTS]: JSON.stringify(many) }), root, log);
  expect(exported(root)).toMatchObject({ hosts: many.slice(0, 16) });
  expect(seen.some((line) => line.includes(HOSTS))).toBe(true);
});

it("drops a value the UI would not accept and logs it", async () => {
  const root = tempRoot();
  const { log, seen } = lines();
  await exportDesktopUi(page({ [THEME]: "neon", [TEXT]: "huge", [DENSITY]: "crowded", [HOSTS]: "{not json", [PENDING]: JSON.stringify({ not: "a list" }) }), root, log);
  expect(exported(root)).toEqual({ version: 1, theme: "dark", textSize: "system", density: "comfortable", hosts: [], pending: [] });
  for (const key of [THEME, TEXT, DENSITY, HOSTS, PENDING]) expect(seen.some((line) => line.includes(key))).toBe(true);
});

it("missing keys give defaults", async () => {
  const root = tempRoot();
  const { log, seen } = lines();
  await exportDesktopUi(page({}), root, log);
  expect(exported(root)).toEqual({ version: 1, theme: "dark", textSize: "system", density: "comfortable", hosts: [], pending: [] });
  // A key the page never set is normal, not a problem to log.
  expect(seen).toEqual([]);
});

it("atomic write leaves no partial file", async () => {
  const root = tempRoot();
  const { log, seen } = lines();
  // A folder where the file belongs makes the final rename fail, after the temporary file was written.
  mkdirSync(exportFile(root), { recursive: true });
  await expect(exportDesktopUi(page({ [THEME]: "light" }), root, log)).resolves.toBeUndefined();
  expect(readdirSync(join(root, "data"))).toEqual(["desktop-ui-export.json"]);
  expect(seen.length).toBeGreaterThan(0);
});

it("a later export replaces the earlier one completely", async () => {
  const root = tempRoot();
  await exportDesktopUi(page({ [THEME]: "light", [HOSTS]: JSON.stringify([host(1), host(2)]) }), root);
  await exportDesktopUi(page({ [DENSITY]: "compact" }), root);
  expect(exported(root)).toEqual({ version: 1, theme: "dark", textSize: "system", density: "compact", hosts: [], pending: [] });
  expect(readdirSync(join(root, "data"))).toEqual(["desktop-ui-export.json"]);
});

it("a read that fails leaves the earlier export alone", async () => {
  const root = tempRoot();
  const { log, seen } = lines();
  await exportDesktopUi(page({ [THEME]: "light", [HOSTS]: JSON.stringify([host(1)]) }), root);
  const before = readFileSync(exportFile(root), "utf8");
  // The page was reloading or had moved off the app's origin: that is not an empty page, so nothing is written.
  await expect(exportDesktopUi(async () => { throw new Error("Execution context was destroyed."); }, root, log)).resolves.toBeUndefined();
  expect(readFileSync(exportFile(root), "utf8")).toBe(before);
  expect(readdirSync(join(root, "data"))).toEqual(["desktop-ui-export.json"]);
  expect(seen.length).toBeGreaterThan(0);
});

// The Rust app reads this file the first time it starts (desktop-host device_settings, `reads_electron_fixture`), so the
// bytes the exporter really writes for a fully filled page are kept beside that crate. Normally the test rewrites the file;
// under CI, or with FIXTURE_CHECK=1, it fails instead when the file is stale.
// R1's patch line has no desktop-host crate, and tjommie-rs/Cargo.toml's members = ["crates/*"] fails on a crates folder with
// no Cargo.toml, so there the test is skipped and writes nothing. It runs where the crate exists (rust-rewrite).
const crate = resolve(import.meta.dirname, "../../../tjommie-rs/crates/desktop-host");
it.skipIf(!existsSync(join(crate, "Cargo.toml")))("writes the cross-implementation fixture", async () => {
  const fixture = join(crate, "tests", "fixtures", "desktop-ui-export.json");
  const root = tempRoot();
  const hosts = [
    host(1, { name: "Desk PC", deviceId: "8d1f4c1e-2b7a-4e0b-b1d3-0a6c5e9f7a22", lan: [{ host: "192.168.1.20", port: 47101 }, { host: "fe80::1", port: 47101 }], relay: "https://relay.example.workers.dev" }),
    host(2, { name: "Laptop" }),
  ];
  await exportDesktopUi(page({ [THEME]: "light", [TEXT]: "larger", [DENSITY]: "compact", [HOSTS]: JSON.stringify(hosts), [PENDING]: JSON.stringify([host(3, { name: "New PC" })]) }), root);
  const produced = readFileSync(exportFile(root), "utf8");
  if (process.env.CI === "true" || process.env.FIXTURE_CHECK === "1") {
    expect(existsSync(fixture), "The fixture is missing. Run this test without CI or FIXTURE_CHECK to write it.").toBe(true);
    // A Windows checkout may have turned the fixture's line endings into CRLF; the bytes that matter are the JSON.
    expect(readFileSync(fixture, "utf8").replace(/\r\n/g, "\n"), "The fixture is stale. Run this test without CI or FIXTURE_CHECK to rewrite it.").toBe(produced);
  } else {
    mkdirSync(dirname(fixture), { recursive: true });
    writeFileSync(fixture, produced);
  }
});
