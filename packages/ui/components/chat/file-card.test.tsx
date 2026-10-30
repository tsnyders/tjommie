import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, expect, it, vi } from "vitest";
import { initialState } from "@/lib/state";
import { update } from "@/lib/store";
import { FileCard } from "./file-card";

afterEach(() => { update(() => initialState); vi.unstubAllGlobals(); });

it("offers local file reveal only while showing this PC", () => {
  vi.stubGlobal("window", { tjommie: { showFile: vi.fn() } });
  const file = { path: "\\\\attacker\\share\\file.txt", name: "file.txt" };
  update((state) => ({ ...state, host: "5b0e2c8e-6f1a-4c55-9a43-2f0d7b1e9c11" }));
  expect(renderToStaticMarkup(<FileCard file={file} />)).not.toContain("Show file.txt in its folder");
  update((state) => ({ ...state, host: null }));
  expect(renderToStaticMarkup(<FileCard file={file} />)).toContain("Show file.txt in its folder");
});
