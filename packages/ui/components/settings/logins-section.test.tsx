// @vitest-environment happy-dom
import { act, type ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { Bot, BrowserState } from "@tjommie/protocol";
import { BrowserLogins, browserLoginTarget } from "../chat/browser-logins";
import { initialState } from "@/lib/state";
import { update } from "@/lib/store";
import { LoginsSection } from "./logins-section";

const call = vi.hoisted(() => vi.fn()), transport = { call };
vi.mock("@/lib/connection", () => ({ daemon: () => transport }));
vi.mock("@/lib/desktop", () => ({ desktop: () => undefined }));
Reflect.set(globalThis, "IS_REACT_ACT_ENVIRONMENT", true);
const entry = { id: "41f8bac7-358c-48cf-9f48-a3a358804da6", name: "Synthetic login", botId: "one", origin: "https://example.com", revision: 3, createdAt: "2026-11-03T00:00:00Z", updatedAt: "2026-11-03T00:00:00Z" };
const state: BrowserState = { botId: "one", generation: "generation-one", control: "human", canControl: true, controlEpoch: 7, privateEpoch: 0, activePageId: "page-one", tabs: [{ id: "page-one", url: "https://example.com/login", title: "Login" }], dialog: null };
let root: Root | undefined;
const settle = () => act(async () => { await new Promise(resolve => setTimeout(resolve, 0)); });
async function mount(node: ReactNode) { root = createRoot(document.body.appendChild(document.createElement("div"))); await act(async () => root!.render(node)); await settle(); }
const button = (label: string): HTMLButtonElement => { const found = [...document.querySelectorAll("button")].find(value => value.textContent === label); if (!found) throw new Error(`Missing ${label}`); return found; };
beforeEach(() => {
  update(() => ({ ...initialState, host: "selected-host", connection: { status: "ready" }, bots: [{ id: "one", name: "Writer" } as Bot] }));
  call.mockImplementation(async (method: string) => method === "vault.list" ? [entry] : method === "vault.status" || method === "vault.unlock" || method === "vault.lock" ? { state: "unlocked", reason: null, pendingDeletes: 0 } : method === "vault.entry" ? { status: "saved", entry } : method === "vault.fill" ? { status: "filled", privateEpoch: 9 } : method === "vault.finish" ? { status: "cleared" } : {});
});
afterEach(async () => { if (root) await act(async () => root!.unmount()); root = undefined; document.body.replaceChildren(); update(() => initialState); call.mockReset(); });
it("shows metadata and invokes native host entry without ordinary secret inputs", async () => {
  await mount(<LoginsSection />); expect(document.body.textContent).toContain(entry.name); expect(document.body.textContent).toContain(entry.origin);
  expect(document.querySelector('input[type="password"]')).toBeNull(); expect(document.querySelector('input[name="username"]')).toBeNull();
  await act(async () => { const select = document.querySelector<HTMLSelectElement>('[aria-label="Bot for new login"]')!; select.value = "one"; select.dispatchEvent(new Event("change", { bubbles: true })); });
  await act(async () => button("New login on host").click()); await settle(); expect(call).toHaveBeenCalledWith("vault.entry", { botId: "one" });
  await act(async () => button("Edit on host").click()); await settle(); expect(call).toHaveBeenCalledWith("vault.entry", { botId: "one", id: entry.id, revision: 3 });
});
it("requires current control/page/epoch, exact-origin metadata, and keeps Finish separate from Resume", async () => {
  const refreshed = vi.fn(); await mount(<BrowserLogins state={state} onRefresh={refreshed} />); await act(async () => button("Saved logins").click()); await settle();
  await act(async () => { const select = document.querySelector<HTMLSelectElement>('[aria-label="Saved login for this origin"]')!; select.value = entry.id; select.dispatchEvent(new Event("change", { bubbles: true })); });
  await act(async () => button("Fill on host").click()); await settle();
  expect(call).toHaveBeenCalledWith("vault.fill", { botId: "one", generation: "generation-one", pageId: "page-one", controlEpoch: 7, id: entry.id, revision: 3 });
  await act(async () => root!.render(<BrowserLogins state={{ ...state, privateActive: true, privateEpoch: 9, tabs: [] }} onRefresh={refreshed} />));
  expect(document.querySelector("select,input")).toBeNull(); expect(document.body.textContent).toContain("Remote fields and pictures are hidden"); expect(document.body.textContent).not.toContain("Resume bot");
  await act(async () => button("Clear private fields on host").click()); await settle(); expect(call).toHaveBeenCalledWith("vault.finish", { botId: "one", generation: "generation-one", privateEpoch: 9, mode: "clear" });
  expect(call.mock.calls.some(([method]) => method === "browser.control")).toBe(false);
  expect(browserLoginTarget({ ...state, canControl: false })).toBeNull(); expect(browserLoginTarget({ ...state, controlEpoch: undefined })).toBeNull(); expect(browserLoginTarget({ ...state, privateActive: true })).toBeNull();
});
it("cancels a pending fill on generation change and discards its late result", async () => {
  let finish!: (value: unknown) => void; const refreshed = vi.fn();
  const regular = call.getMockImplementation()!; call.mockImplementation((method, params) => method === "vault.fill" ? new Promise(resolve => { finish = resolve; }) : regular(method, params));
  await mount(<BrowserLogins state={state} onRefresh={refreshed} />); await act(async () => button("Saved logins").click()); await settle();
  await act(async () => { const select = document.querySelector<HTMLSelectElement>("select")!; select.value = entry.id; select.dispatchEvent(new Event("change", { bubbles: true })); }); await act(async () => button("Fill on host").click());
  await act(async () => root!.render(<BrowserLogins state={{ ...state, generation: "generation-two" }} onRefresh={refreshed} />));
  expect(call).toHaveBeenCalledWith("vault.cancel", {}); await act(async () => finish({ status: "filled", privateEpoch: 9 })); await settle(); expect(refreshed).not.toHaveBeenCalled(); expect(document.body.textContent).not.toContain("Login filled privately");
});
