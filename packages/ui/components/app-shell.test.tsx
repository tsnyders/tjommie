import { renderToStaticMarkup } from "react-dom/server";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { initialState, type AppState } from "@/lib/state";
import { update } from "@/lib/store";
import { AppShell, StatusPill } from "./app-shell";

vi.mock("next/navigation", () => ({ usePathname: () => "/agents/", useRouter: () => ({ push: () => {} }), useSearchParams: () => new URLSearchParams() }));
// Server rendering reads a store's server snapshot (the initial state); these tests render the state they set.
vi.mock("@/lib/store", async (original) => {
  const store = await original<typeof import("@/lib/store")>();
  return { ...store, useApp: <T,>(select: (state: AppState) => T): T => select(store.getState()) };
});

beforeEach(() => update(() => initialState));

describe("StatusPill", () => {
  it("says it's reconnecting, with a spinner, in a pill that takes no clicks so the screen under it still works", () => {
    const out = renderToStaticMarkup(<StatusPill connection={{ status: "reconnecting" }} />);
    expect(out).toContain("Reconnecting…");
    expect(out).toContain('role="status"');
    expect(out).toContain("animate-spin");
    expect(out).toContain("pointer-events-none");
  });

  it("says the PC is offline when its relay says so, and that it can't be reached otherwise, without a spinner", () => {
    const offline = renderToStaticMarkup(<StatusPill connection={{ status: "offline", reason: "host_offline" }} />);
    expect(offline).toContain("Your PC is offline");
    expect(offline).not.toContain("animate-spin");
    expect(offline).toContain("pointer-events-none");
    const unreachable = renderToStaticMarkup(<StatusPill connection={{ status: "offline", reason: "unreachable" }} />);
    expect(unreachable).toContain("Can&#x27;t reach your PC");
    expect(unreachable).not.toContain("Your PC is offline");
  });

  it("shows nothing while the connection is up", () => {
    expect(renderToStaticMarkup(<StatusPill connection={{ status: "ready" }} />)).toBe("");
  });
});

describe("AppShell", () => {
  it("keeps the cached screens on show under the pill while the PC can't be reached", () => {
    update((state) => ({ ...state, connection: { status: "offline", reason: "unreachable" } }));
    const out = renderToStaticMarkup(<AppShell><p>Cached chat</p></AppShell>);
    expect(out).toContain("Cached chat");
    expect(out).toContain("Can&#x27;t reach your PC");
  });

  it("puts the removed screen in place of the cached screens", () => {
    update((state) => ({ ...state, connection: { status: "removed" } }));
    const out = renderToStaticMarkup(<AppShell><p>Cached chat</p></AppShell>);
    expect(out).not.toContain("Cached chat");
    expect(out).toContain("This PC removed this device");
  });
});
