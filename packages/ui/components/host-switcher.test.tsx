import { renderToStaticMarkup } from "react-dom/server";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { initialState, type AppState, type Connection } from "@/lib/state";
import { update } from "@/lib/store";
import { HostSwitcher, THIS_PC, chooseHost, hostRows } from "./host-switcher";

const HOME = "5b0e2c8e-6f1a-4c55-9a43-2f0d7b1e9c11", WORK = "9c4f6b1d-2a7e-4e3b-8d55-0f6a1c2b3d4e";
const hosts = [{ id: HOME, name: "Home PC" }, { id: WORK, name: "Work PC" }];

const lib = vi.hoisted(() => ({ connectRemote: vi.fn(), connectLocal: vi.fn(), forgetRemote: vi.fn(async () => {}) }));
vi.mock("@/lib/connection", () => lib);
vi.mock("@/lib/remote-hosts", async (original) => ({ ...(await original<typeof import("@/lib/remote-hosts")>()), remoteHosts: () => hosts }));
vi.mock("@/lib/store", async (original) => {
  const store = await original<typeof import("@/lib/store")>();
  return { ...store, useApp: <T,>(select: (state: AppState) => T): T => select(store.getState()) };
});

beforeEach(() => {
  vi.clearAllMocks();
  update(() => initialState);
});

describe("hostRows", () => {
  it("lists This PC first on the desktop, then each saved PC with how it's doing", () => {
    expect(hostRows(hosts, null, { status: "ready" }, true)).toEqual([
      { value: THIS_PC, label: "This PC", status: null },
      { value: HOME, label: "Home PC", status: "Not connected" },
      { value: WORK, label: "Work PC", status: "Not connected" },
    ]);
  });

  it("gives the PC in use the connection's state, and the others Not connected", () => {
    const status = (connection: Connection) => hostRows(hosts, HOME, connection, true)[1].status;
    expect(status({ status: "ready" })).toBe("Connected");
    expect(status({ status: "connecting" })).toBe("Connecting");
    expect(status({ status: "reconnecting" })).toBe("Connecting");
    expect(status({ status: "offline", reason: "host_offline" })).toBe("Offline");
    expect(status({ status: "offline", reason: "unreachable" })).toBe("Offline");
    expect(status({ status: "removed" })).toBe("Not connected");
    expect(hostRows(hosts, HOME, { status: "ready" }, true)[2].status).toBe("Not connected");
  });

  it("has no This PC on a phone, which has no Tjommie of its own", () => {
    expect(hostRows(hosts, HOME, { status: "ready" }, false).map((row) => row.label)).toEqual(["Home PC", "Work PC"]);
  });
});

describe("chooseHost", () => {
  it("connects to the saved PC picked", () => {
    chooseHost(WORK, HOME);
    expect(lib.connectRemote).toHaveBeenCalledWith(WORK);
    expect(lib.connectLocal).not.toHaveBeenCalled();
  });

  it("goes back to this PC's own Tjommie on This PC", () => {
    chooseHost(THIS_PC, HOME);
    expect(lib.connectLocal).toHaveBeenCalledTimes(1);
    expect(lib.connectRemote).not.toHaveBeenCalled();
  });

  it("leaves the PC in use alone when it's picked again, so its link isn't dropped", () => {
    chooseHost(HOME, HOME);
    chooseHost(THIS_PC, null);
    expect(lib.connectRemote).not.toHaveBeenCalled();
    expect(lib.connectLocal).not.toHaveBeenCalled();
  });
});

describe("HostSwitcher", () => {
  it("is the rail's avatar on this PC: a menu button with the account's initial", () => {
    const out = renderToStaticMarkup(<HostSwitcher initial="T" />);
    expect(out).toContain('aria-label="Switch PC, now on This PC"');
    expect(out).toContain('aria-haspopup="menu"');
    expect(out).toContain(">T<");
  });

  it("shows the PC's name in the button while on another PC", () => {
    update((state) => ({ ...state, host: HOME }));
    const out = renderToStaticMarkup(<HostSwitcher initial="T" />);
    expect(out).toContain('aria-label="Switch PC, now on Home PC"');
    expect(out).toContain(">Home PC<");
  });

  it("is the Switch PC pill on a phone", () => {
    const out = renderToStaticMarkup(<HostSwitcher phone />);
    expect(out).toContain(">Switch PC<");
    expect(out).toContain('aria-haspopup="menu"');
  });
});
