// @vitest-environment happy-dom
import { act, StrictMode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AccountInfo, Bot } from "@tjommie/protocol";
import type { Banner } from "@/lib/accounts";
import { initialState } from "@/lib/state";
import { update } from "@/lib/store";
import { BannerView } from "./setup-banner";
import { SignInNotifications } from "./sign-in-toast";

vi.mock("@/lib/connection", () => ({ request: vi.fn(), daemon: vi.fn() }));
Reflect.set(globalThis, "IS_REACT_ACT_ENVIRONMENT", true);
const bot = { id: "bot", provider: "claude", name: "Scout" } as Bot;
const account = { id: "personal", label: "Claude", provider: "claude", kind: "subscription", setup: { signedIn: false } } as AccountInfo;
let root: Root;
let container: HTMLDivElement;
let banner: Banner | null;
const settle = () => act(async () => { await new Promise((resolve) => setTimeout(resolve, 0)); });
async function render() {
  await act(async () => root.render(<StrictMode><SignInNotifications>{banner && <BannerView bot={bot} banner={banner} />}</SignInNotifications></StrictMode>));
  await settle();
}

beforeEach(() => {
  update(() => ({ ...initialState, host: "pc-one", connection: { status: "ready" }, accounts: [account] }));
  container = document.body.appendChild(document.createElement("div"));
  root = createRoot(container);
  banner = { kind: "signIn", account };
});
afterEach(async () => {
  await act(async () => root.unmount());
  document.body.replaceChildren();
  update(() => initialState);
});

describe("sign-in notifications", () => {
  it("shows one dismissible toast in Strict Mode with an explicit account-settings action", async () => {
    await render();
    expect(document.querySelectorAll('[data-slot="sign-in-toast"]')).toHaveLength(1);
    expect(container.textContent).not.toContain("isn't signed in");
    expect(document.body.textContent).toContain("Claude isn't signed in.");
    expect(document.querySelector<HTMLAnchorElement>('[data-slot="sign-in-toast"] a')?.getAttribute("href")).toContain("section=accounts&account=personal&action=signin");
    expect(document.querySelector('[aria-label="Dismiss sign-in notification"]')).not.toBeNull();
  });

  it("keeps a dismissed notice closed across status refreshes and revisiting the same account", async () => {
    await render();
    await act(async () => document.querySelector<HTMLButtonElement>('[aria-label="Dismiss sign-in notification"]')!.click());
    await settle();
    banner = null; await render();
    banner = { kind: "signIn", account: { ...account } }; await render();
    await act(async () => update((state) => ({ ...state, accounts: [{ ...account }], connection: { status: "ready" } })));
    expect(document.querySelector('[data-slot="sign-in-toast"]')).toBeNull();
  });

  it("shows a new failure after successful sign-in and keeps different PCs separate", async () => {
    await render();
    banner = null; await render();
    await act(async () => update((state) => ({ ...state, accounts: [{ ...account, setup: { ...account.setup!, signedIn: true } }] })));
    await act(async () => update((state) => ({ ...state, accounts: [account] })));
    banner = { kind: "signIn", account }; await render();
    expect(document.querySelector('[data-slot="sign-in-toast"]')).not.toBeNull();
    await act(async () => update((state) => ({ ...state, host: "pc-two" })));
    await settle();
    expect(document.querySelectorAll('[data-slot="sign-in-toast"]')).toHaveLength(1);
  });

  it("also turns legacy provider sign-in guidance into a toast", async () => {
    banner = { kind: "setup", signIn: true, sandbox: false, text: "Claude isn't signed in for Tjommie yet." };
    await render();
    expect(container.textContent).not.toContain("isn't signed in");
    expect(document.querySelector('[data-slot="sign-in-toast"]')?.textContent).toContain("Claude isn't signed in for Tjommie yet.");
    expect(document.querySelector<HTMLAnchorElement>('[data-slot="sign-in-toast"] a')?.getAttribute("href")).toContain("section=accounts");
  });
});
