import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { abilities, attemptProblem, canClearLocal, canRead, cardLine, googleStep, needsReconsent, serviceList, validClientId, watchGoogle, type GoogleApi, type GoogleStatus } from "./google-connection";

const base: GoogleStatus = { configured: true, connected: false, email: null, services: [], grantedBotIds: [], attempt: null };
const connected: GoogleStatus = { ...base, connected: true, email: "me@example.com", services: ["gmail"], grantedBotIds: ["b1"] };

describe("google status", () => {
  it("is set up, connect, waiting or connected", () => {
    expect(googleStep({ ...base, configured: false })).toBe("setup");
    expect(googleStep(base)).toBe("connect");
    expect(googleStep({ ...base, attempt: { id: "a1", state: "waiting" } })).toBe("waiting");
    expect(googleStep({ ...base, attempt: { id: "a1", state: "failed", message: "Denied" } })).toBe("connect");
    expect(googleStep(connected)).toBe("connected");
  });

  it("gives access only to the bots granted it, and none by default", () => {
    expect(canRead(connected, "b1")).toBe(true);
    expect(canRead(connected, "b2")).toBe(false);
    expect(canRead({ ...connected, grantedBotIds: [] }, "b1")).toBe(false);
  });

  it("says why a sign-in failed", () => {
    expect(attemptProblem({ ...base, attempt: { id: "a1", state: "failed", message: "Access denied." } })).toBe("Access denied.");
    expect(attemptProblem({ ...base, attempt: { id: "a1", state: "failed" } })).toContain("didn't finish");
    expect(attemptProblem({ ...base, attempt: { id: "a1", state: "waiting" } })).toBeNull();
  });

  it("describes itself for the bot panel's card", () => {
    expect(cardLine(null, "Scout", "b1")).toBe("Checking…");
    expect(cardLine({ ...base, configured: false }, "Scout", "b1")).toBe("Not set up yet");
    expect(cardLine(connected, "Scout", "b1")).toBe("me@example.com. Scout has full access to Gmail.");
    expect(cardLine(connected, "Scout", "b2")).toBe("me@example.com. Scout has no access.");
  });
});

describe("services and clearing", () => {
  it("lists services in words, and before any is connected names what will be asked for", () => {
    expect([serviceList(["gmail"]), serviceList(["gmail", "drive"]), serviceList(["gmail", "drive", "calendar"])]).toEqual(["Gmail", "Gmail and Drive", "Gmail, Drive and Calendar"]);
    expect(serviceList([])).toBe("Gmail, Drive and Calendar");
  });
  it("says what full access allows for the services covered", () => {
    expect(abilities(["gmail"])).toBe("send and manage your email");
    expect(abilities([])).toBe("send and manage your email, create, edit and delete Drive files and create, edit and delete Calendar events");
  });
  it("needs a new consent only for a connection the daemon says is read-only", () => {
    expect(needsReconsent({ ...connected, access: "read" } as GoogleStatus)).toBe(true);
    expect(needsReconsent({ ...connected, access: "full" } as GoogleStatus)).toBe(false);
    expect(needsReconsent(connected)).toBe(false);
    expect(needsReconsent({ ...base, access: "read" } as GoogleStatus)).toBe(false);
    expect(cardLine({ ...connected, access: "read" } as GoogleStatus, "Scout", "b1")).toBe("me@example.com. Reconnect to give full access.");
  });
  it("can clear local settings only while not connected and something is saved or wrong", () => {
    expect(canClearLocal({ ...base, configured: false })).toBe(false);
    expect(canClearLocal(base)).toBe(true);
    expect(canClearLocal({ ...base, configured: false, attempt: { id: "load", state: "failed" } })).toBe(true);
    expect(canClearLocal(connected)).toBe(false);
  });
});

describe("validClientId", () => {
  it("accepts a Google client ID and nothing else", () => {
    expect(validClientId(" 123-abc_d.apps.googleusercontent.com ")).toBe(true);
    expect(validClientId("123-abc")).toBe(false);
    expect(validClientId("evil.test/x.apps.googleusercontent.com")).toBe(false);
  });
});

describe("watchGoogle", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  function fake(statuses: GoogleStatus[]) {
    let at = 0;
    let listener: ((status: GoogleStatus) => void) | null = null;
    const status = vi.fn(async () => statuses[Math.min(at++, statuses.length - 1)]);
    const api = { status, onChanged: (next: (status: GoogleStatus) => void) => ((listener = next), () => (listener = null)) } as unknown as GoogleApi;
    return { api, status, push: (value: GoogleStatus) => listener?.(value), listening: () => listener !== null };
  }
  const waiting: GoogleStatus = { ...base, attempt: { id: "a1", state: "waiting" } };

  it("reads once when nothing is waiting", async () => {
    const t = fake([base]);
    const seen: GoogleStatus[] = [];
    watchGoogle(t.api, (status) => seen.push(status), () => {});
    await vi.advanceTimersByTimeAsync(5000);
    expect(t.status).toHaveBeenCalledTimes(1);
    expect(seen).toEqual([base]);
  });

  it("polls every second while a sign-in is waiting, and stops when it's done", async () => {
    const t = fake([waiting, waiting, connected]);
    const seen: GoogleStatus[] = [];
    watchGoogle(t.api, (status) => seen.push(status), () => {});
    await vi.advanceTimersByTimeAsync(3500);
    expect(t.status).toHaveBeenCalledTimes(3);
    expect(seen.at(-1)).toEqual(connected);
  });

  it("follows the daemon's change events, and stops following when stopped", async () => {
    const t = fake([base]);
    const seen: GoogleStatus[] = [];
    const stop = watchGoogle(t.api, (status) => seen.push(status), () => {});
    await vi.advanceTimersByTimeAsync(0);
    t.push(connected);
    expect(seen.at(-1)).toEqual(connected);
    stop();
    expect(t.listening()).toBe(false);
    t.push(base);
    expect(seen.at(-1)).toEqual(connected);
  });

  it("reports a failed read", async () => {
    const api = { status: async () => Promise.reject(new Error("offline")), onChanged: () => () => {} } as unknown as GoogleApi;
    const errors: unknown[] = [];
    watchGoogle(api, () => {}, (error) => errors.push(error));
    await vi.advanceTimersByTimeAsync(0);
    expect(errors).toHaveLength(1);
  });
});
