import { describe, expect, it } from "vitest";
import { NativeBrowserAccess } from "./native-browser-access.ts";

describe("native browser controller and private epochs", () => {
  it("rejects a second controller and old input across takeover and page changes", () => {
    const access = new NativeBrowserAccess();
    access.take("device-a"); const first = access.controlEpoch;
    expect(() => access.take("device-b")).toThrow("Another device");
    expect(() => access.require("device-b", first)).toThrow("control changed");
    access.changedPage();
    expect(() => access.require("device-a", first)).toThrow("control changed");
    expect(access.release("device-b")).toBe(false);
    expect(access.release("device-a")).toBe(true);
    access.take("device-b");
    expect(() => access.require("device-a", first)).toThrow();
  });
  it("fences queued and late results, and stays private after disconnect or hiding", () => {
    const access = new NativeBrowserAccess(), before = access.ticket();
    access.take("device-a");
    expect(() => access.check(before, true)).toThrow();
    const capture = access.ticket(), epoch = access.beginPrivate("device-a", access.controlEpoch);
    expect(() => access.check(capture)).toThrow();
    access.release("device-a");
    expect(access.control).toBe("human"); expect(access.privateActive).toBe(true);
    expect(() => access.check(access.ticket(), true)).toThrow();
    expect(() => access.endPrivate("device-b", epoch)).toThrow();
    access.take("device-b"); access.endPrivate("device-b", epoch);
    expect(() => access.check(capture)).toThrow();
    expect(access.control).toBe("human");
    access.release("device-b"); expect(access.control).toBe("bot");
  });
});
