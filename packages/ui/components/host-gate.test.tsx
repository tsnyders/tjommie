// @vitest-environment happy-dom
import { act, useState, type ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Bot } from "@tjommie/protocol";
import BotSettingsPage from "@/app/bots/settings/page";
import type { DesktopApi } from "@/lib/desktop";
import { googleApi } from "@/lib/google-api";
import type { GoogleApi, GoogleStatus } from "@/lib/google-connection";
import { initialState } from "@/lib/state";
import { update } from "@/lib/store";
import { FolderRow } from "./bot-form/rows";
import { ChatHeader } from "./chat/chat-header";
import { Composer } from "./chat/composer";
import { FileCard } from "./chat/file-card";
import { RightPanel } from "./chat/right-panel";
import { GoogleCard } from "./google/google-connection";

// What acts on this PC (its files and folders, its folder picker, its browser, its Google sign-in, attaching its files to a message) is offered
// only while the window shows this PC, and changes the moment the window moves to another PC or back.

Reflect.set(globalThis, "IS_REACT_ACT_ENVIRONMENT", true);
const HOME = vi.hoisted(() => "5b0e2c8e-6f1a-4c55-9a43-2f0d7b1e9c11");
vi.mock("next/navigation", () => ({ useRouter: () => ({ push: () => {}, replace: () => {}, back: () => {} }), useSearchParams: () => new URLSearchParams("bot=b1") }));
const sent = vi.hoisted(() => [] as unknown[]); // what each chat.send carried
vi.mock("@/lib/connection", () => ({
  daemon: () => ({ call: async (method: string, params: unknown) => { if (method === "chat.send") sent.push(params); return {}; } }),
  request: vi.fn(() => new Promise(() => {})),
}));
vi.mock("@/lib/remote-hosts", async (original) => ({ ...(await original<typeof import("@/lib/remote-hosts")>()), remoteHosts: () => [{ id: HOME, name: "Home PC" }] }));

const bot: Bot = {
  id: "b1", name: "Scout", color: "blue", shape: "circle", instructions: "", provider: "codex", model: "gpt-5", accountId: null, autonomy: "ask", homeFolder: "C:\\bots\\b1",
  projectFolder: null, browser: true, paused: false, canMessage: [], state: "idle", setupReason: null, limitedUntil: null, createdAt: "2026-10-25T08:00:00.000Z",
};
// This PC's bridge, as the desktop app's preload exposes it.
const bridge = {
  nativeBrowser: {}, showFile: vi.fn(), openFolder: vi.fn(async () => {}), openNotes: vi.fn(async () => {}), pickFolder: vi.fn(async () => "C:\\work"),
  connectGoogle: vi.fn(async () => ({ attempt: "a1" })), pickFiles: vi.fn(async () => ["C:\\out\\report.pdf"]), pathForFile: vi.fn(() => "C:\\out\\dropped.txt"),
};

const roots: Root[] = [];
async function mount(node: ReactNode): Promise<void> {
  const root = createRoot(document.body.appendChild(document.createElement("div")));
  roots.push(root);
  await act(async () => root.render(node));
}
const settle = (): Promise<void> => act(async () => { await new Promise((resolve) => setTimeout(resolve, 0)); });
// The window moves to another PC (its id) or back to this one (null), as the host switcher does.
const showPc = (host: string | null): Promise<void> => act(async () => update((state) => ({ ...state, host })));
const labelled = (label: string): HTMLElement | null => document.querySelector(`[aria-label="${label}"]`);
const button = (text: string): HTMLButtonElement | undefined => [...document.querySelectorAll("button")].find((each) => each.textContent === text);
const text = (): string => document.body.textContent ?? "";
// What a screen reader says after a control's name: the text its aria-describedby points to.
const description = (element: Element): string | null => {
  const ids = element.getAttribute("aria-describedby");
  return ids ? ids.split(" ").map((id) => document.getElementById(id)?.textContent ?? "").join(" ") : null;
};

beforeEach(() => {
  vi.clearAllMocks();
  sent.length = 0;
  window.tjommie = bridge as unknown as DesktopApi;
});
afterEach(async () => {
  for (const root of roots.splice(0)) await act(async () => root.unmount());
  document.body.replaceChildren();
  delete window.tjommie;
  update(() => initialState);
});

describe("this PC's buttons", () => {
  it("go the moment the window moves to another PC, and come back with this PC", async () => {
    await mount(
      <>
        <FileCard file={{ path: "C:\\out\\report.pdf", name: "report.pdf" }} />
        <RightPanel bot={bot} messages={[]} pane="details" onPane={() => {}} expanded={false} onExpandedChange={() => {}} room={1400} overlay={false} onHide={() => {}} />
      </>,
    );
    const mine = ["Show report.pdf in its folder", "Open the home folder", "Bot panel views", "Open browser"];
    for (const label of mine) expect(labelled(label), label).not.toBeNull();
    await showPc(HOME);
    for (const label of mine) expect(labelled(label), label).toBeNull();
    expect(text()).toContain("Runs on Home PC. Open Tjommie on that PC to watch Scout or take over.");
    await showPc(null);
    for (const label of mine) expect(labelled(label), label).not.toBeNull();
  });

  it("leave the chat's More menu while it's open", async () => {
    await mount(<ChatHeader bot={bot} panelOpen={false} onTogglePanel={() => {}} onOpenBrowser={() => {}} />);
    await act(async () => { labelled("More")?.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, button: 0 })); });
    expect(text()).toContain("Open home folder");
    expect(text()).toContain("Open browser");
    await showPc(HOME);
    expect(text()).toContain("Bot settings"); // still open
    expect(text()).not.toContain("Open home folder");
    expect(text()).not.toContain("Open browser");
  });

  it("leave Bot settings' About this bot", async () => {
    update((state) => ({ ...state, bots: [bot] }));
    await mount(<BotSettingsPage />);
    expect(button("Open folder")).toBeDefined();
    expect(button("Open notes.md")).toBeDefined();
    await showPc(HOME);
    expect(button("Open folder")).toBeUndefined();
    expect(button("Open notes.md")).toBeUndefined();
  });
});

describe("Google on another PC", () => {
  const status: GoogleStatus = { configured: true, connected: false, email: null, services: [], grantedBotIds: [], attempt: null };
  const fake = (): GoogleApi => ({
    status: vi.fn(async () => status), configure: vi.fn(async () => status), connect: vi.fn(async () => ({ attempt: "a1" })), cancel: vi.fn(async () => ({})),
    disconnect: vi.fn(async () => status), grant: vi.fn(async () => status), onChanged: () => () => {},
  });

  it("says Google runs on that PC, as the browser card does, with nothing to sign in to here", async () => {
    update((state) => ({ ...state, connection: { status: "ready" }, host: HOME }));
    const api = fake();
    await mount(<GoogleCard bot={bot} api={api} />);
    expect(text()).toContain("Runs on Home PC. Open Tjommie on that PC to connect or manage Google.");
    expect(labelled("Manage Google")).toBeNull();
    expect(document.querySelector("section button, section [aria-disabled]")).toBeNull(); // no unavailable control: the reason is the card's own text
    await showPc(null);
    await settle();
    expect(labelled("Manage Google")?.textContent).toBe("Connect");
    await showPc(HOME);
    expect(labelled("Manage Google")).toBeNull();
    expect(api.connect).not.toHaveBeenCalled();
  });

  it("never begins a sign-in on this PC's daemon for it", async () => {
    update((state) => ({ ...state, host: HOME }));
    await expect(googleApi.connect()).rejects.toThrow("Connect Google in Tjommie on the PC it's for.");
    expect(bridge.connectGoogle).not.toHaveBeenCalled();
    update((state) => ({ ...state, host: null }));
    await expect(googleApi.connect()).resolves.toEqual({ attempt: "a1" });
    expect(bridge.connectGoogle).toHaveBeenCalledOnce();
  });
});

describe("the bot form's project folder", () => {
  function Form() {
    const [folder, setFolder] = useState<string | null>(null);
    return <FolderRow projectFolder={folder} onChange={setFolder} />;
  }
  const radio = (label: string): HTMLElement | undefined => [...document.querySelectorAll<HTMLElement>('[role="radio"]')].find((each) => each.textContent === label);
  const type = (input: HTMLInputElement, value: string): Promise<void> =>
    act(async () => {
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set?.call(input, value);
      input.dispatchEvent(new Event("input", { bubbles: true }));
    });

  it("on another PC is typed, as a path on that PC, and never opens this PC's picker", async () => {
    update((state) => ({ ...state, host: HOME }));
    await mount(<Form />);
    await act(async () => document.querySelector<HTMLElement>("button[aria-expanded]")?.click());
    await act(async () => radio("A project folder")?.click());
    await settle();
    expect(bridge.pickFolder).not.toHaveBeenCalled();
    const input = labelled("Project folder path");
    if (!(input instanceof HTMLInputElement)) throw new Error("No path to type");
    expect(input.disabled).toBe(false); // nothing here is unavailable: the path is typed, with the reason shown under it and linked to it
    expect(description(input)).toBe("A folder on Home PC. Type its full path.");
    await type(input, "D:\\Sites\\shop");
    expect(input.value).toBe("D:\\Sites\\shop");
    expect(radio("A project folder")?.getAttribute("aria-checked")).toBe("true");
  });

  it("on this PC is picked, and the window moving between PCs swaps the picker and the typed path at once", async () => {
    await mount(<Form />);
    await act(async () => document.querySelector<HTMLElement>("button[aria-expanded]")?.click());
    await act(async () => radio("A project folder")?.click());
    await settle();
    expect(bridge.pickFolder).toHaveBeenCalledOnce();
    expect(text()).toContain("C:\\work");
    expect(labelled("Project folder path")).toBeNull();
    await showPc(HOME);
    expect(button("Change")).toBeUndefined();
    expect((labelled("Project folder path") as HTMLInputElement | null)?.value).toBe("C:\\work");
    await showPc(null);
    expect(labelled("Project folder path")).toBeNull();
    await act(async () => button("Change")?.click());
    expect(bridge.pickFolder).toHaveBeenCalledTimes(2);
  });
});

describe("the composer's attachments", () => {
  // A file dragged in from Explorer, as the window's drop sees it.
  const drop = (): Promise<void> =>
    act(async () => {
      const event = new Event("drop", { bubbles: true, cancelable: true });
      Object.defineProperty(event, "dataTransfer", { value: { types: ["Files"], files: [new File(["x"], "dropped.txt")] } });
      document.querySelector("form")?.dispatchEvent(event);
    });
  const send = async (message: string): Promise<void> => {
    const field = labelled("Message Scout");
    if (!(field instanceof HTMLTextAreaElement)) throw new Error("No message field");
    await act(async () => {
      Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")?.set?.call(field, message);
      field.dispatchEvent(new Event("input", { bubbles: true }));
    });
    await act(async () => { field.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true })); });
    await settle();
  };
  const attachButton = (): HTMLButtonElement => {
    const found = labelled("Attach files");
    if (!(found instanceof HTMLButtonElement)) throw new Error("No attach button");
    return found;
  };
  const alert = (): string | null | undefined => document.querySelector('[role="alert"]')?.textContent;

  it("on another PC are off, as on a phone: no path goes with a message, the button names that PC, and a drop says so", async () => {
    update((state) => ({ ...state, host: HOME }));
    await mount(<Composer bot={bot} messages={[]} />);
    await act(async () => attachButton().click());
    await settle();
    await drop();
    await send("Here it is");
    expect(sent).toEqual([{ botId: "b1", text: "Here it is", files: [] }]);
    expect(bridge.pickFiles).not.toHaveBeenCalled();
    expect(bridge.pathForFile).not.toHaveBeenCalled();
    expect(labelled("Attached files")).toBeNull();
    expect(attachButton().title).toBe("Attach files on Home PC");
    await drop();
    expect(alert()).toBe("Attach files on Home PC.");
  });

  it("on another PC keep the Attach button reachable, marked unavailable, with the PC-specific reason to read and on press", async () => {
    update((state) => ({ ...state, host: HOME }));
    await mount(<Composer bot={bot} messages={[]} />);
    const attach = attachButton();
    expect(attach.disabled).toBe(false); // a native disabled button would drop out of the keyboard's way, reason and all
    expect(attach.getAttribute("aria-disabled")).toBe("true");
    expect(description(attach)).toBe("Attach files on Home PC");
    attach.focus();
    expect(document.activeElement).toBe(attach);
    await act(async () => attach.click()); // Enter and Space press it the same way
    await settle();
    expect(bridge.pickFiles).not.toHaveBeenCalled();
    expect(alert()).toBe("Attach files on Home PC.");
    await showPc(null);
    expect(attachButton().getAttribute("aria-disabled")).toBe("false");
    expect(description(attachButton())).toBeNull();
  });

  it("on this PC carry its paths, and none of them once the window shows another PC", async () => {
    await mount(<Composer bot={bot} messages={[]} />);
    expect(attachButton().getAttribute("aria-disabled")).toBe("false");
    await act(async () => attachButton().click());
    await settle();
    await drop();
    expect(labelled("Attached files")?.textContent).toContain("report.pdf");
    expect(labelled("Attached files")?.textContent).toContain("dropped.txt");
    await showPc(HOME); // attached on this PC, then the window moved
    await send("For the other PC");
    expect(sent).toEqual([{ botId: "b1", text: "For the other PC", files: [] }]);
    expect(attachButton().getAttribute("aria-disabled")).toBe("true");
    expect(labelled("Attached files")).toBeNull(); // what shows is what goes
    await showPc(null);
    await send("For this PC");
    expect(sent).toEqual([
      { botId: "b1", text: "For the other PC", files: [] },
      { botId: "b1", text: "For this PC", files: ["C:\\out\\report.pdf", "C:\\out\\dropped.txt"] },
    ]);
  });
});
