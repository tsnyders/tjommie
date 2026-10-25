// @vitest-environment happy-dom
import { act, type ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { initialState, type AppState } from "@/lib/state";
import { update } from "@/lib/store";
import { ConnectionScreen, RemovedScreen } from "./connection-screen";
import { clearPresses, press } from "./press-fixture";

Reflect.set(globalThis, "IS_REACT_ACT_ENVIRONMENT", true);
const roots: Root[] = [];
// Renders into the page, as the app does; render() swaps what's shown, the way the app replaces a screen.
async function mount(node: ReactNode): Promise<{ host: HTMLElement; render: (next: ReactNode) => Promise<void> }> {
  const host = document.body.appendChild(document.createElement("div"));
  const root = createRoot(host);
  roots.push(root);
  const render = (next: ReactNode): Promise<void> => act(async () => root.render(next));
  await render(node);
  return { host, render };
}
const settle = (): Promise<void> => act(async () => { await new Promise((resolve) => setTimeout(resolve, 0)); });
const button = (host: HTMLElement, text: string): HTMLButtonElement => {
  const found = [...host.querySelectorAll("button")].find((each) => each.textContent === text);
  if (!found) throw new Error(`No button "${text}"`);
  return found;
};
afterEach(async () => {
  for (const root of roots.splice(0)) await act(async () => root.unmount());
  document.body.replaceChildren();
});

const lib = vi.hoisted(() => ({ connectRemote: vi.fn(), connectLocal: vi.fn(), forgetRemote: vi.fn(async () => {}), restartDaemon: vi.fn() }));
vi.mock("@/lib/connection", () => lib);
vi.mock("@/lib/store", async (original) => {
  const store = await original<typeof import("@/lib/store")>();
  return { ...store, useApp: <T,>(select: (state: AppState) => T): T => select(store.getState()) };
});
vi.mock("@/components/pill", async (original) => (await import("@/components/press-fixture")).recordingPill(await original()));
vi.mock("@/lib/remote-hosts", async (original) => ({ ...(await original<typeof import("@/lib/remote-hosts")>()), remoteHosts: () => [{ id: HOME, name: "Home PC" }] }));

const HOME = vi.hoisted(() => "5b0e2c8e-6f1a-4c55-9a43-2f0d7b1e9c11");

beforeEach(() => {
  clearPresses();
  vi.clearAllMocks();
  update(() => ({ ...initialState, connection: { status: "removed" }, host: HOME }));
});

describe("RemovedScreen", () => {
  it("says the PC removed this device, and offers Pair again and Forget", () => {
    const out = renderToStaticMarkup(<RemovedScreen android={false} />);
    expect(out).toContain("This PC removed this device");
    expect(out).toContain(">Pair again<");
    expect(out).toContain(">Forget<");
  });

  it("forgets that PC on Forget", () => {
    renderToStaticMarkup(<RemovedScreen android={false} />);
    press("Forget");
    expect(lib.forgetRemote).toHaveBeenCalledWith(HOME);
  });

  it("takes a phone to its connect screen to pair again, keeping the PC saved", () => {
    renderToStaticMarkup(<RemovedScreen android />);
    press("Pair again");
    expect(lib.connectRemote).toHaveBeenCalledWith("");
    expect(lib.forgetRemote).not.toHaveBeenCalled();
  });

  it("takes the focus to its heading when it replaces the screen, so the removal is read out and the keyboard starts there", async () => {
    const { host, render } = await mount(<button type="button">Send</button>);
    button(host, "Send").focus();
    expect(document.activeElement?.textContent).toBe("Send");
    await render(<RemovedScreen android={false} />);
    expect(document.activeElement?.tagName).toBe("H1");
    expect(document.activeElement?.textContent).toBe("This PC removed this device");
  });

  it("stays, and says why, when the PC can't be forgotten", async () => {
    lib.forgetRemote.mockRejectedValueOnce(new Error("Windows couldn't delete this PC's key."));
    const { host } = await mount(<RemovedScreen android={false} />);
    await act(async () => button(host, "Forget").click());
    await settle();
    expect(host.querySelector('[role="alert"]')?.textContent).toBe("Windows couldn't delete this PC's key.");
    expect(host.querySelector("h1")?.textContent).toBe("This PC removed this device");
    expect(lib.forgetRemote).toHaveBeenCalledWith(HOME);
  });
});

describe("ConnectionScreen", () => {
  it("shows the removed screen when the PC refused this device", () => {
    expect(renderToStaticMarkup(<ConnectionScreen connection={{ status: "removed" }} />)).toContain("This PC removed this device");
  });

  it("names the other PC the desktop is connecting to, instead of starting its own Tjommie", () => {
    update((state) => ({ ...state, connection: { status: "connecting" } }));
    const out = renderToStaticMarkup(<ConnectionScreen connection={{ status: "connecting" }} />);
    expect(out).toContain("Connecting to Home PC");
    expect(out).not.toContain("Starting Tjommie");
  });

  it("says it can't connect to the other PC, why, and tries that PC again", () => {
    const out = renderToStaticMarkup(<ConnectionScreen connection={{ status: "failed", reason: "Can't reach your PC. Check that it's awake with Tjommie open." }} />);
    expect(out).toContain("Can&#x27;t connect to Home PC");
    expect(out).toContain("Check that it&#x27;s awake with Tjommie open.");
    expect(out).not.toContain("background service");
    press("Try again");
    expect(lib.restartDaemon).toHaveBeenCalledTimes(1); // which retries that PC
  });

  it("keeps the desktop's own words for this PC", () => {
    update((state) => ({ ...state, host: null }));
    expect(renderToStaticMarkup(<ConnectionScreen connection={{ status: "connecting" }} />)).toContain("Starting Tjommie");
    expect(renderToStaticMarkup(<ConnectionScreen connection={{ status: "failed", reason: "x" }} />)).toContain("Tjommie&#x27;s background service didn&#x27;t start");
  });
});
