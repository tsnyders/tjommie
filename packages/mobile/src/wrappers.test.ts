import { describe, expect, it, vi } from "vitest";
import { codeFromScan } from "./scan.ts";
import { createPlugins, fromBase64, MAX_FRAME_BYTES, PLUGIN_NAMES, toBase64 } from "./wrappers.ts";

describe("plugin registration", () => {
  it("registers the three native plugins by their names", () => {
    const register = vi.fn((name: string) => ({ name }));
    const plugins = createPlugins(<T,>(name: string) => register(name) as T);
    expect(register.mock.calls.map(([name]) => name)).toEqual(["TjommieSecureStore", "TjommieSocket", "TjommiePairingScanner"]);
    expect(PLUGIN_NAMES.socket).toBe("TjommieSocket");
    expect(plugins.socket).toEqual({ name: "TjommieSocket" });
  });
});

describe("push availability", () => {
  it("reports unavailable from the native build flag without registering for Firebase", async () => {
    const { createPush } = await import("./wrappers.ts");
    const register = vi.fn(async () => {});
    const push = createPush({ available: async () => ({ value: false }) }, {
      register,
      checkPermissions: async () => ({ receive: "granted" as const }),
      requestPermissions: async () => ({ receive: "granted" as const }),
      addListener: vi.fn(async () => ({ remove: async () => {} })),
    } as unknown as Parameters<typeof createPush>[1]);
    expect(await push.available()).toBe(false);
    await expect(push.register()).rejects.toThrow("Push isn't in this build.");
    expect(register).not.toHaveBeenCalled();
  });
});

describe("base64 for the bridge", () => {
  it("round-trips any bytes, including ones over a chunk and non-text ones", () => {
    const bytes = Uint8Array.from({ length: 70_000 }, (_, at) => at % 256);
    expect(fromBase64(toBase64(bytes))).toEqual(bytes);
    expect(toBase64(new Uint8Array([0, 255, 128]))).toBe("AP+A");
    expect(fromBase64("")).toEqual(new Uint8Array());
  });

  it("holds the frame limit the native side enforces", () => {
    expect(MAX_FRAME_BYTES).toBe(65535);
  });
});

describe("codeFromScan", () => {
  const code = "A".repeat(40);

  it("takes the code out of the pairing link, or from a bare code", () => {
    expect(codeFromScan(`tjommie://pair#${code}`)).toBe(code);
    expect(codeFromScan(`  tjommie://pair#${code}\n`)).toBe(code);
    expect(codeFromScan(code)).toBe(code);
  });

  it("ignores a cancelled scan and anything that is not a code", () => {
    expect(codeFromScan(null)).toBeNull();
    expect(codeFromScan("")).toBeNull();
    expect(codeFromScan("https://example.com/")).toBeNull();
    expect(codeFromScan("tjommie://pair#short")).toBeNull();
    expect(codeFromScan(`tjommie://pair#${code} extra`)).toBeNull();
    expect(codeFromScan("tjommie://other#" + code)).toBeNull();
  });
});
