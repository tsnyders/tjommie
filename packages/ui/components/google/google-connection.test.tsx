import type { Bot } from "@tjommie/protocol";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import type { GoogleStatus } from "@/lib/google-connection";
import { GoogleBody } from "./google-connection";

const bot = { id: "b1", name: "Scout" } as Bot;
const base: GoogleStatus = { configured: true, connected: false, email: null, services: [], grantedBotIds: [], attempt: null };
const connected: GoogleStatus = { ...base, connected: true, email: "me@example.com", services: ["gmail"], grantedBotIds: [] };
const noop = vi.fn();
const html = (status: GoogleStatus, problem: string | null = null, busy = false) =>
  renderToStaticMarkup(<GoogleBody bot={bot} status={status} busy={busy} problem={problem} onConfigure={noop} onConnect={noop} onCancel={noop} onGrant={noop} onDisconnect={noop} onClear={noop} />);

describe("GoogleBody clearing and services", () => {
  const unreadable: GoogleStatus = { ...base, configured: false, attempt: { id: "load", state: "failed", message: "Disconnect Google to reset." } };

  it("offers Clear local Google settings when the saved settings can't be read, and on any failed attempt while not connected", () => {
    expect(html(unreadable)).toContain("Clear local Google settings");
    expect(html({ ...base, attempt: { id: "x", state: "failed", message: "Retry disconnect." } })).toContain("Clear local Google settings");
    expect(html(base)).toContain("Clear local Google settings"); // saved, so it can be started over
  });

  it("doesn't offer it with nothing saved and nothing wrong, or while connected", () => {
    expect(html({ ...base, configured: false })).not.toContain("Clear local Google settings");
    expect(html(connected)).not.toContain("Clear local Google settings");
  });

  it("describes full access to every connected service, and says nothing is read-only", () => {
    const all = html({ ...connected, services: ["gmail", "drive", "calendar"] });
    expect(all).toContain("Allow Scout full access to Gmail, Drive and Calendar");
    expect(all).toContain("send and manage your email, create, edit and delete Drive files and create, edit and delete Calendar events");
    expect(all).not.toContain("read-only");
    expect(all).not.toContain("can&#x27;t send");
    expect(html({ ...connected, services: ["gmail"] })).toContain("Allow Scout full access to Gmail");
    expect(html(base)).toContain("full access to Gmail, Drive and Calendar");
  });

  it("asks a read-only connection to reconnect before any bot can be allowed", () => {
    const old = html({ ...connected, access: "read" } as GoogleStatus);
    expect(old).toContain("Reconnect Google");
    expect(old).toContain("read-only access");
    expect(old).toMatch(/type="checkbox"[^>]*disabled=""|disabled=""[^>]*type="checkbox"/);
    expect(html(connected)).not.toContain("Reconnect Google");
  });
});

describe("GoogleBody", () => {
  it("always says what a bot can do with it, and that it isn't the built-in browser's sign-in", () => {
    for (const status of [{ ...base, configured: false }, base, connected]) {
      const out = html(status);
      expect(out).toContain("A bot you allow can send and manage your email");
      expect(out).toContain("doesn&#x27;t sign the built-in browser in to Google");
    }
  });

  it("asks for the app's client first, with the secret in a password field", () => {
    const out = html({ ...base, configured: false });
    expect(out).toContain("Client ID");
    expect(out).toMatch(/id="google-client-secret"[^>]*type="password"|type="password"[^>]*id="google-client-secret"/);
    expect(out).toContain('autoComplete="off"');
    expect(out).toContain("Connecting Google");
    expect(out).toContain("Save");
    expect(out).not.toContain("Connect Google");
  });

  it("offers Connect Google once configured, and Cancel while waiting", () => {
    expect(html(base)).toContain("Connect Google");
    const waiting = html({ ...base, attempt: { id: "a1", state: "waiting" } });
    expect(waiting).toContain("Finish signing in in your browser");
    expect(waiting).toContain("Cancel");
    expect(waiting).not.toContain("Connect Google");
  });

  it("shows why a sign-in failed, and lets the user try again", () => {
    const out = html({ ...base, attempt: { id: "a1", state: "failed", message: "Access denied." } });
    expect(out).toContain("Access denied.");
    expect(out).toContain("Connect Google");
  });

  it("shows the account, an unchecked Gmail box for this bot, and Disconnect", () => {
    const out = html(connected);
    expect(out).toContain("me@example.com");
    expect(out).toContain("Allow Scout full access to Gmail");
    expect(out).not.toMatch(/type="checkbox"[^>]*checked=""/);
    expect(out).toContain("Disconnect Google");
  });

  it("checks the box only for a bot that was granted, and disables it without Gmail connected", () => {
    expect(html({ ...connected, grantedBotIds: ["b1"] })).toMatch(/type="checkbox"[^>]*checked=""|checked=""[^>]*type="checkbox"/);
    expect(html({ ...connected, services: [] })).toMatch(/type="checkbox"[^>]*disabled=""|disabled=""[^>]*type="checkbox"/);
  });

  it("shows a problem from the last action", () => {
    expect(html(connected, "Google said no.")).toContain("Google said no.");
  });
});
