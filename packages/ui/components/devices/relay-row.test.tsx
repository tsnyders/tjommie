import type { RemoteStatus } from "@tjommie/protocol";
import { DEFAULT_RELAY_URL } from "@tjommie/protocol";
import type { ComponentProps } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { clearPresses, press } from "../press-fixture";
import { RelayRow, saveRelayUrl } from "./relay-row";

const requested = vi.hoisted(() => vi.fn<(...call: unknown[]) => Promise<object>>(async () => ({})));
vi.mock("@/lib/connection", () => ({ request: requested }));
vi.mock("@/components/pill", async (original) => (await import("@/components/press-fixture")).recordingPill(await original()));

const OWN = "https://relay.example.dev";
const PUBLIC = DEFAULT_RELAY_URL;
const relay = (state: RemoteStatus["relay"]["state"], push = false): RemoteStatus["relay"] => ({ url: OWN, state, push });
const html = (props: Partial<ComponentProps<typeof RelayRow>> = {}) => renderToStaticMarkup(<RelayRow saved={OWN} relay={relay("online", true)} {...props} />);

beforeEach(() => {
  clearPresses();
  requested.mockClear();
});

describe("RelayRow", () => {
  it("names the relay and holds its https address", () => {
    const out = html();
    expect(out).toContain(">Relay<");
    expect(out).toMatch(/<input[^>]*type="url"/);
    expect(out).toContain(`value="${OWN}"`);
    expect(out).toContain('placeholder="https://');
  });

  it("says how the relay is doing: Connected, Connecting, Can't reach the relay, or Off", () => {
    expect(html({ relay: relay("online") })).toContain("Connected");
    expect(html({ relay: relay("connecting") })).toContain("Connecting");
    expect(html({ relay: relay("error") })).toContain("Can&#x27;t reach the relay");
    expect(html({ saved: null, relay: { url: null, state: "off", push: false } })).toContain("Off");
  });

  it("says nothing about the relay's state until the PC has said", () => {
    const out = html({ relay: null });
    for (const state of ["Connected", "Connecting", "Can&#x27;t reach the relay", "Off", "Push:"]) expect(out).not.toContain(state);
  });

  it("says whether push works through it once it's connected", () => {
    expect(html({ relay: relay("online", true) })).toContain("Push: available");
    expect(html({ relay: relay("online", false) })).toContain("Push: not set up on this relay");
    expect(html({ relay: relay("connecting") })).not.toContain("Push:");
  });

  it("offers the public relay only when there is one and it isn't the one in use", () => {
    expect(html()).toContain("Use the public relay");
    expect(html({ publicUrl: null })).not.toContain("Use the public relay");
    expect(html({ saved: PUBLIC })).not.toContain("Use the public relay");
  });

  it("saves the public relay's address with Use the public relay", () => {
    html();
    press("Use the public relay");
    expect(requested).toHaveBeenCalledWith("settings.set", { relayUrl: PUBLIC });
  });

  it("keeps Save off until the address is changed", () => {
    expect(html()).toMatch(/<button[^>]*type="submit"[^>]*disabled=""[^>]*>Save</);
  });
});

describe("saveRelayUrl", () => {
  it("saves a typed https address, trimmed", async () => {
    expect(await saveRelayUrl("  https://relay.example.dev  ")).toBeNull();
    expect(requested).toHaveBeenCalledWith("settings.set", { relayUrl: "https://relay.example.dev" });
  });

  it("turns the relay off when the address is cleared", async () => {
    expect(await saveRelayUrl("   ")).toBeNull();
    expect(requested).toHaveBeenCalledWith("settings.set", { relayUrl: null });
  });

  it("refuses anything but an https address, and saves nothing", async () => {
    for (const typed of ["http://relay.example.dev", "relay.example.dev", "wss://relay.example.dev"]) expect(await saveRelayUrl(typed)).toBe("Use an https:// address.");
    expect(requested).not.toHaveBeenCalled();
  });

  it("says why a save failed", async () => {
    requested.mockRejectedValueOnce(new Error("Tjommie isn't connected right now."));
    expect(await saveRelayUrl(OWN)).toBe("Tjommie isn't connected right now.");
  });
});
