import { expect, it, vi } from "vitest";

const registerPlugin = vi.hoisted(() => vi.fn(() => ({ start: async () => {}, stop: async () => {} })));
vi.mock("@capacitor/core", () => ({ registerPlugin }));
vi.mock("./remote-hosts", () => ({ isAndroidApp: () => true }));

it("never registers a native keep-alive plugin after the service was dropped", async () => {
  const { keepAliveRunning, syncKeepAlive } = await import("./keep-alive");
  syncKeepAlive("ready", "Home PC");
  syncKeepAlive("outside");
  expect(keepAliveRunning()).toBe(false);
  expect(registerPlugin).not.toHaveBeenCalledWith("TjommieKeepAlive");
});
