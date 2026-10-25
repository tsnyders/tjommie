import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import { MobileConnect, type MobileConnectApi, type MobileHostView } from "./mobile-connect";

const api = (hosts: MobileHostView[] = []): MobileConnectApi => ({ hosts: () => hosts, pair: vi.fn(), connect: vi.fn(), forget: vi.fn(), scan: vi.fn() });
const html = (status: "outside" | "failed" | "connecting", hosts: MobileHostView[] = [], reason?: string) => renderToStaticMarkup(<MobileConnect status={status} reason={reason} api={api(hosts)} />);

describe("MobileConnect", () => {
  it("starts a phone with no PC by asking to pair, in honest words: same Wi-Fi, PC awake, nothing connected", () => {
    const out = html("outside");
    expect(out).toContain("Pair with your PC");
    expect(out).toContain("same Wi-Fi");
    expect(out).toContain("awake");
    expect(out).toContain("Settings, then Devices, then Pair a device");
    expect(out).not.toContain("Saved PCs");
    expect(out.toLowerCase()).not.toContain("connected to your");
    expect(out.toLowerCase()).not.toContain("mobile data");
  });

  it("lists the PCs it was paired with, each with Connect and Forget by name", () => {
    const out = html("outside", [{ id: "h1", name: "Home PC" }, { id: "h2", name: "Work PC" }]);
    expect(out).toContain("Connect to your PC");
    expect(out).toContain("Saved PCs");
    for (const name of ["Home PC", "Work PC"]) {
      expect(out).toContain(`aria-label="Connect to ${name}"`);
      expect(out).toContain(`aria-label="Forget ${name}"`);
    }
    expect(out).toContain("Pair another PC");
  });

  it("holds the pairing code in a box the keyboard is told to leave alone, and cannot connect until a good code is in it", () => {
    const out = html("outside");
    expect(out).toMatch(/<textarea[^>]*id="pair-code"/);
    for (const attribute of ['autoComplete="off"', 'autoCapitalize="off"', 'autoCorrect="off"', 'spellCheck="false"']) expect(out).toContain(attribute);
    expect(out).toMatch(/<button[^>]*disabled=""[^>]*>Connect<\/button>|<button[^>]*>Connect<\/button>/);
    expect(out).toMatch(/disabled=""[^>]*>Connect</);
    expect(out).toContain("Scan code");
    expect(out).toContain("Name for this phone");
    expect(out).toContain('value="Android phone"');
  });

  it("says why it could not start when the connection failed", () => {
    expect(html("failed", [], "Tjommie couldn't reach your PC.")).toContain("Tjommie couldn&#x27;t reach your PC.");
  });

  it("shows it is connecting, with a spinner and the choices off, and doesn't claim success", () => {
    const out = html("connecting", [{ id: "h1", name: "Home PC" }]);
    expect(out).toContain("Connecting to your PC");
    expect(out).toContain("animate-spin");
    expect(out).toMatch(/disabled=""[^>]*aria-label="Connect to Home PC"|aria-label="Connect to Home PC"[^>]*disabled=""/);
    expect(out).not.toContain("Connected");
  });

  it("says a device reaches the PC on the same Wi-Fi, or from anywhere through the PC's relay", () => {
    const out = html("outside");
    expect(out).toContain("same Wi-Fi");
    expect(out).toContain("relay");
    expect(out).toContain("from anywhere");
  });

  it("lets another PC connect by pasting the code: no scanner, named Another PC until renamed", () => {
    const out = renderToStaticMarkup(<MobileConnect status="outside" device="pc" api={api()} />);
    expect(out).not.toContain("Scan code");
    expect(out).toContain("Name for this PC");
    expect(out).toContain('value="Another PC"');
    expect(out).toContain("On the other PC open Settings, then Devices, then Pair a device");
    expect(out).toMatch(/<textarea[^>]*id="pair-code"/);
  });

  it("fits in a dialog, whose title is its heading", () => {
    const out = renderToStaticMarkup(<MobileConnect status="outside" device="pc" dialog api={api([{ id: "h1", name: "Home PC" }])} />);
    expect(out).not.toContain("<h1");
    expect(out).toContain('aria-label="Connect to Home PC"');
    expect(out).toContain("Pair another PC");
  });
});
