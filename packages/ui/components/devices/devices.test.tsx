import type { Device } from "@tjommie/protocol";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import { DeviceRow, DevicesSection, RenameBody } from "./devices-section";
import { PairBody } from "./pair-dialog";

vi.mock("@/lib/connection", () => ({ request: vi.fn(), daemon: () => ({ subscribe: () => () => {} }) }));

const device = (patch: Partial<Device> = {}): Device => ({ id: "11111111-1111-4111-8111-111111111111", name: "My phone", publicKey: "A".repeat(43), pairedAt: "2026-10-20T09:00:00.000Z", lastSeenAt: null, revokedAt: null, ...patch });

describe("DevicesSection", () => {
  const out = renderToStaticMarkup(<DevicesSection />);

  it("says what pairing means, honestly: full control, same Wi-Fi, PC awake, this PC's window always works, no relay or push claimed", () => {
    expect(out).toContain("Every paired device has full control of Tjommie, including pairing and revoking devices.");
    expect(out).toContain("same Wi-Fi");
    expect(out).toContain("has to be awake");
    expect(out).toContain("This PC&#x27;s own");
    expect(out.toLowerCase()).not.toContain("relay");
    expect(out.toLowerCase()).not.toContain("push");
    expect(out.toLowerCase()).not.toContain("anywhere");
  });

  it("has Pair a device as an explicit button, and makes no code on its own", () => {
    expect(out).toContain("Pair a device");
    expect(out).not.toContain("Pairing QR code");
    expect(out).not.toContain("pairing-code");
  });
});

describe("DeviceRow", () => {
  it("shows the name, when it was paired and last seen, and Rename and Revoke by name", () => {
    const out = renderToStaticMarkup(<DeviceRow device={device({ lastSeenAt: new Date(Date.now() - 3 * 3_600_000).toISOString() })} connected={false} onRename={() => {}} onRevoke={() => {}} />);
    expect(out).toContain("My phone");
    expect(out).toContain("Paired");
    expect(out).toContain("Last seen 3 hours ago");
    expect(out).toContain('aria-label="Rename My phone"');
    expect(out).toContain('aria-label="Revoke My phone"');
  });

  it("says Connected now for a device on the line, and Not seen yet for one that never connected", () => {
    expect(renderToStaticMarkup(<DeviceRow device={device()} connected onRename={() => {}} onRevoke={() => {}} />)).toContain("Connected now");
    expect(renderToStaticMarkup(<DeviceRow device={device()} connected={false} onRename={() => {}} onRevoke={() => {}} />)).toContain("Not seen yet");
  });
});

describe("PairBody", () => {
  const out = renderToStaticMarkup(<PairBody onClose={() => {}} />);

  it("makes nothing until Show pairing code is pressed: no QR, no code, just the explanation and the button", () => {
    expect(out).toContain("Show pairing code");
    expect(out).toContain("works once and lasts ten minutes");
    expect(out).toContain("same Wi-Fi");
    expect(out).not.toContain("Pairing QR code");
    expect(out).not.toContain("pairing-code");
    expect(out).not.toContain("Copy code");
    expect(out).toContain(">Close<");
  });
});

describe("RenameBody", () => {
  it("starts with the device's name and keeps Save off until it changes", () => {
    const out = renderToStaticMarkup(<RenameBody device={device()} onClose={() => {}} onDone={() => {}} />);
    expect(out).toContain('value="My phone"');
    expect(out).toMatch(/disabled=""[^>]*>Save<|<button[^>]*type="submit"[^>]*disabled=""/);
  });
});
