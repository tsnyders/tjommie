// @vitest-environment happy-dom
import { act, type ComponentProps } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import SettingsPage from "@/app/settings/page";

const query = vi.hoisted(() => ({ value: "" }));
vi.mock("next/navigation", () => ({ useSearchParams: () => new URLSearchParams(query.value), useRouter: () => ({ back: () => {} }) }));
vi.mock("next/link", () => ({ default: ({ children, ...props }: ComponentProps<"a">) => <a {...props}>{children}</a> }));
vi.mock("@/components/settings/device-settings", () => ({ GeneralSection: () => <p>Current-device general controls</p>, NotificationsSection: () => <p>Device notifications</p>, FilesSection: () => <p>Device files</p> }));
vi.mock("@/components/settings/appearance-section", () => ({ AppearanceSection: () => <p>Device appearance</p> }));
vi.mock("@/components/settings/voice-section", () => ({ VoiceSection: () => <p>Device voice</p> }));
vi.mock("@/components/settings/diagnostics-section", () => ({ DiagnosticsSection: () => <p>Safe report</p>, AboutSection: () => <p>Device and host versions</p> }));
vi.mock("@/components/accounts/accounts-section", () => ({ AccountsSection: ({ account, action }: { account: string | null; action: string | null }) => <p>account={account};action={action}</p> }));
vi.mock("@/components/usage/usage-section", () => ({ UsageSection: () => <p>Host usage</p> }));
vi.mock("@/components/devices/devices-section", () => ({ DevicesSection: () => <p>Host devices</p> }));

beforeEach(() => { query.value = ""; });
Reflect.set(globalThis, "IS_REACT_ACT_ENVIRONMENT", true);
const roots: Root[] = [];
async function html(): Promise<string> {
  const host = document.body.appendChild(document.createElement("div"));
  const root = createRoot(host); roots.push(root);
  await act(async () => root.render(<SettingsPage />));
  return host.innerHTML;
}
afterEach(async () => { for (const root of roots.splice(0)) await act(async () => root.unmount()); document.body.replaceChildren(); });

describe("settings section links", () => {
  it("keeps all existing deep links and adds the device sections", async () => {
    const output = await html();
    for (const section of ["general", "bots", "accounts", "usage", "devices", "appearance", "notifications", "files", "voice", "diagnostics", "about"]) expect(output).toContain(`/settings/?section=${section}`);
    expect(output).toContain("Current-device general controls");
    expect(output).toContain('aria-label="Settings"');
  });
  it("preserves account targeting and only the supported account actions", async () => {
    for (const action of ["signin", "cap", "retry"]) {
      query.value = `section=accounts&account=account-2&action=${action}`;
      expect(await html()).toContain(`account=account-2;action=${action}`);
    }
    query.value = "section=accounts&account=account-2&action=delete";
    expect(await html()).toContain("account=account-2;action=");
    expect(await html()).not.toContain("action=delete");
  });
  it("routes old host sections and returns unknown sections to General", async () => {
    for (const [section, content] of [["devices", "Host devices"], ["usage", "Host usage"], ["bots", "Requests between agents per chain"], ["appearance", "Device appearance"], ["voice", "Device voice"], ["diagnostics", "Safe report"], ["about", "Device and host versions"], ["bogus", "Current-device general controls"]]) {
      query.value = `section=${section}`;
      expect(await html()).toContain(content);
    }
    expect(await html()).toContain("Back to Settings");
  });
});
