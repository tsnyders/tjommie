// @vitest-environment happy-dom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";
import { FilesSection } from "./device-settings";

const invalidate = vi.hoisted(() => vi.fn(async () => {}));
vi.mock("@/lib/attachment-device", () => ({ attachmentDevice: () => ({ invalidate }) }));
Reflect.set(globalThis, "IS_REACT_ACT_ENVIRONMENT", true);
const roots: Root[] = [];
async function mount(): Promise<void> {
  const root = createRoot(document.body.appendChild(document.createElement("div"))); roots.push(root);
  await act(async () => root.render(<FilesSection />));
}
afterEach(async () => {
  for (const root of roots.splice(0)) await act(async () => root.unmount());
  document.body.replaceChildren(); invalidate.mockReset();
});

describe("current-device temporary file data", () => {
  it("describes cancellation and preservation, and clears only via the native file-session API", async () => {
    await mount();
    expect(document.body.textContent).toContain("cancels unfinished downloads");
    expect(document.body.textContent).toContain("Saved downloads, delivered host files, chats, accounts and worktrees are kept");
    expect(invalidate).not.toHaveBeenCalled();
    await act(async () => document.querySelector<HTMLButtonElement>("button")?.click());
    expect(invalidate).toHaveBeenCalledOnce();
    expect(document.body.textContent).toContain("Temporary file data cleared on this device.");
  });
  it("does not claim cleanup succeeded when the native API fails", async () => {
    invalidate.mockRejectedValueOnce(new Error("SYNTHETIC_PRIVATE_PATH"));
    await mount();
    await act(async () => document.querySelector<HTMLButtonElement>("button")?.click());
    expect(document.body.textContent).toContain("Temporary file data could not be cleared");
    expect(document.body.textContent).not.toContain("SYNTHETIC_PRIVATE_PATH");
    expect(document.body.textContent).not.toContain("data cleared");
  });
});
