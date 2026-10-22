import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { afterAll, describe, expect, it } from "vitest";
import { navigationFor, uiFile } from "./app-protocol.ts";

function exportFolder(): string {
  const root = mkdtempSync(join(tmpdir(), "tjommie ui ñ-"));
  const ui = join(root, "ui");
  mkdirSync(ui, { recursive: true });
  for (const file of [
    "index.html",
    "404.html",
    "chat/index.html",
    "chat/index.txt",
    "__next._tree.txt",
    "__next._full.txt",
    "__next.__PAGE__.txt",
    "settings/__next._tree.txt",
    "settings/__next._full.txt",
    "settings/__next.settings/__PAGE__.txt",
    "bots/new/__next.bots/new/__PAGE__.txt",
    "_next/static/chunks/app.js",
  ]) {
    mkdirSync(dirname(join(ui, file)), { recursive: true });
    writeFileSync(join(ui, file), file);
  }
  return ui;
}

describe("uiFile", () => {
  const ui = exportFolder();
  afterAll(() => rmSync(dirname(ui), { recursive: true, force: true, maxRetries: 5, retryDelay: 100 }));

  it("maps pages, page data and assets to files in the static export", () => {
    expect(uiFile(ui, "app://tjommie/")).toBe(join(ui, "index.html"));
    expect(uiFile(ui, "app://tjommie/chat/?bot=b1")).toBe(join(ui, "chat", "index.html"));
    expect(uiFile(ui, "app://tjommie/chat")).toBe(join(ui, "chat", "index.html"));
    expect(uiFile(ui, "app://tjommie/chat/index.txt?_rsc=1x2y")).toBe(join(ui, "chat", "index.txt"));
    expect(uiFile(ui, "app://tjommie/_next/static/chunks/app.js")).toBe(join(ui, "_next", "static", "chunks", "app.js"));
  });

  it("keeps existing flat segment data names", () => {
    expect(uiFile(ui, "app://tjommie/__next._tree.txt")).toBe(join(ui, "__next._tree.txt"));
    expect(uiFile(ui, "app://tjommie/__next._full.txt")).toBe(join(ui, "__next._full.txt"));
    expect(uiFile(ui, "app://tjommie/__next.__PAGE__.txt")).toBe(join(ui, "__next.__PAGE__.txt"));
    expect(uiFile(ui, "app://tjommie/settings/__next._tree.txt")).toBe(join(ui, "settings", "__next._tree.txt"));
    expect(uiFile(ui, "app://tjommie/settings/__next._full.txt")).toBe(join(ui, "settings", "__next._full.txt"));
  });

  it("maps nested page segment requests to the export's folder form", () => {
    expect(uiFile(ui, "app://tjommie/settings/__next.settings.__PAGE__.txt")).toBe(
      join(ui, "settings", "__next.settings", "__PAGE__.txt"),
    );
    expect(uiFile(ui, "app://tjommie/bots/new/__next.bots.new.__PAGE__.txt?_rsc=abc")).toBe(
      join(ui, "bots", "new", "__next.bots", "new", "__PAGE__.txt"),
    );
  });

  it("refuses paths that leave the export, and missing files", () => {
    expect(uiFile(ui, "app://tjommie/..%5C..%5Csecret.txt")).toBeNull();
    expect(uiFile(ui, "app://tjommie/settings/..%5C..%5C__next.settings.__PAGE__.txt")).toBeNull();
    expect(uiFile(ui, "app://tjommie/%E0%A4%A")).toBeNull();
    expect(uiFile(ui, "app://tjommie/missing/")).toBeNull();
    expect(uiFile(ui, "app://evil/chat/")).toBeNull();
    expect(uiFile(ui, "https://tjommie/chat/")).toBeNull();
  });

  it("rejects encoded traversal to a sibling file outside the export", () => {
    writeFileSync(join(dirname(ui), "secret.txt"), "outside the UI export");
    expect(uiFile(ui, "app://tjommie/..%2Fsecret.txt")).toBeNull();
  });
});

describe("navigationFor", () => {
  it("keeps Tjommie's own pages in the window and sends web links to the browser (navigation stays in the app)", () => {
    expect(navigationFor("app://tjommie/chat/?bot=b1", true)).toBe("stay");
    expect(navigationFor("https://pnet.co.za/jobs", true)).toBe("browser");
    expect(navigationFor("http://localhost:3000/chat/", true)).toBe("browser");
    expect(navigationFor("http://localhost:3000/chat/", false)).toBe("stay");
    expect(navigationFor("app://tjommie/chat/", false)).toBe("block");
    expect(navigationFor("file:///C:/Windows/win.ini", true)).toBe("block");
    expect(navigationFor("javascript:alert(1)", true)).toBe("block");
    expect(navigationFor("app://evil/", true)).toBe("block");
  });
});
