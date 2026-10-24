import { mkdirSync } from "node:fs";
import { join, resolve } from "node:path";
import { expect, it } from "vitest";
import { openDb } from "../../daemon/src/db.ts";
import { recordUsage } from "../../daemon/src/usage.ts";
import { closeRealApp, eventually, launchRealApp, openChat, type RealApp } from "./real-app.ts";

async function capture(app: RealApp, path: string): Promise<void> {
  // A minimized Electron window has no compositor frame. Restore only this fixture.
  await app.electronApp.evaluate(({ BrowserWindow }) => {
    const window = BrowserWindow.getAllWindows()[0];
    if (window.isMinimized()) window.restore();
    if (!window.isVisible()) window.show();
  });
  await app.page.bringToFront();
  await app.page.screenshot({ path, animations: "disabled" });
}

// Real DOM/main/preload/daemon with isolated, unsigned provider profiles. No sign-in or paid turn.
it("edits account metadata, pins an existing bot and shows separate spend and subscription value", async () => {
  let app: RealApp | undefined;
  const syntheticKeys = new Set<string>();
  try {
    app = await launchRealApp(undefined, { isolatedAccounts: true });
    const { page, client } = app;
    const accounts = await client.call("accounts.list", {});
    const original = accounts.find((account) => account.provider === "claude" && account.isDefault);
    if (!original) throw new Error("Isolated default account missing.");
    const other = await client.call("accounts.create", { provider: "claude", label: "UI secondary", kind: "subscription" });
    const bot = await client.call("bots.create", { name: "Account UI fixture", color: "blue", shape: "circle", provider: "claude", model: "sonnet", instructions: "No provider calls in this fixture", autonomy: "ask", browser: false });
    await page.getByRole("link", { name: "Settings", exact: true }).click();
    await page.getByRole("link", { name: "Accounts", exact: true }).click();
    await page.getByRole("button", { name: "More for UI secondary" }).click();
    await page.getByRole("menuitem", { name: "Rename", exact: true }).click();
    const rename = page.getByRole("dialog", { name: "Rename account", exact: true });
    await rename.getByLabel("Name", { exact: true }).fill("UI renamed");
    await rename.getByRole("button", { name: "Save", exact: true }).click();
    await page.getByRole("button", { name: "More for UI renamed" }).click();
    await page.getByRole("menuitem", { name: "Make default", exact: true }).click();
    await eventually(async () => (await client.call("accounts.list", {})).find((account) => account.id === other.id && account.isDefault));
    expect((await client.call("bots.list", {})).find((entry) => entry.id === bot.id)?.accountId).toBe(original.id);
    const screenshots = resolve(import.meta.dirname, "../../../docs/screenshots/phase2");
    mkdirSync(screenshots, { recursive: true });
    await capture(app, join(screenshots, "accounts-desktop.png"));

    await openChat(page, bot.name);
    await page.getByRole("link", { name: "Bot settings", exact: true }).click();
    await page.getByRole("button", { name: /^Account and model/ }).click();
    await page.getByRole("button", { name: /UI renamed/ }).click();
    await page.keyboard.press("Escape");
    await page.getByRole("button", { name: "Save changes", exact: true }).click();
    await eventually(async () => (await client.call("bots.list", {})).find((entry) => entry.id === bot.id && entry.accountId === other.id));

    // This key is fake; only its uniquely namespaced disposable WinCred entry is exercised.
    const key = await client.call("accounts.create", { provider: "claude", label: "UI synthetic key", kind: "api_key", apiKey: "synthetic-only-ui-account-fixture", spendingCapUsd: 10 });
    syntheticKeys.add(key.id);
    const db = openDb(join(app.root, "data", "tjommie.db"));
    try {
      for (const [accountId, costUsd] of [[other.id, 2], [key.id, 1]] as const) {
        recordUsage(db, { botId: bot.id, accountId, provider: "claude", model: "sonnet", input: 100, cachedInput: 10, output: 20, reasoning: 5, costUsd, costSource: "reported" });
      }
    } finally { db.close(); }
    await page.getByRole("link", { name: "Settings", exact: true }).click();
    await page.getByRole("link", { name: "Usage", exact: true }).click();
    await page.getByText("Spent (API keys)", { exact: true }).waitFor();
    await page.getByText("$1.00", { exact: true }).waitFor();
    await page.getByText("$2.00", { exact: true }).waitFor();
    await page.getByRole("radio", { name: "By account", exact: true }).click();
    await page.getByRole("list").getByText("UI renamed", { exact: true }).waitFor();
    await page.getByRole("list").getByText("UI synthetic key", { exact: true }).waitFor();
    expect(await page.evaluate("document.documentElement.scrollWidth <= innerWidth")).toBe(true);
    await capture(app, join(screenshots, "usage-desktop.png"));
  } finally {
    try {
      // Clear synthetic WinCred entries before deleting the fixture data root.
      for (const id of syntheticKeys) await app?.client.call("accounts.remove", { id });
    } finally { await closeRealApp(app, false); }
  }
}, 120_000);
