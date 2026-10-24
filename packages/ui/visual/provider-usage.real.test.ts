import { mkdirSync, readdirSync, readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { chromium } from "playwright-core";
import { expect, it } from "vitest";
import { AccountInfo, type AccountProviderUsage } from "@tjommie/protocol";
import { ProviderUsage } from "../components/usage/provider-usage";

// Render the actual provider-card component/CSS with deliberately synthetic metadata.
// This checks its visual bounds without using any provider credentials or network.
it("renders provider allowance cards, credit units and stale readings at phone widths", async () => {
  const now = new Date("2026-10-22T12:00:00.000Z");
  const reading = (provider: "claude" | "codex"): AccountProviderUsage => ({
    status: "available", checkedAt: now.toISOString(), lastSuccessfulAt: now.toISOString(), error: null,
    planLabel: provider === "claude" ? "Max 20x" : "Pro",
    windows: provider === "claude" ? [
      { id: "session", label: "Session", scope: null, durationMinutes: 300, usedPercent: 8, resetsAt: "2026-10-22T14:00:00.000Z" },
      { id: "weekly", label: "Weekly", scope: null, durationMinutes: 10080, usedPercent: 5, resetsAt: "2026-10-28T12:00:00.000Z" },
      { id: "model", label: "Weekly", scope: { kind: "model", id: "fixture-model", label: "Fable" }, durationMinutes: 10080, usedPercent: null, resetsAt: null },
    ] : [{ id: "weekly", label: "Weekly", scope: null, durationMinutes: 10080, usedPercent: 31, resetsAt: "2026-10-26T12:00:00.000Z" }],
    credits: provider === "codex" ? { balance: "38917.83", hasCredits: true, unlimited: false } : null,
    extraUsageEnabled: provider === "claude" ? false : null,
  });
  const accounts = (["claude", "codex"] as const).map((provider) => AccountInfo.parse({
    id: `fixture-${provider}`, provider, label: `${provider === "claude" ? "Claude" : "Codex"} sample account`, kind: "subscription", isDefault: true,
    isFallback: false, spendingCapUsd: null, createdAt: now.toISOString(), hasCredential: false, limits: [], signIn: null,
    spending: { knownUsd: 0, unknownRows: 0, capUsd: null, blocked: false },
    setup: { provider, installed: true, version: "0.0.0", signedIn: true, account: "sample@example.invalid", plan: null, error: null, ...(provider === "codex" ? { sandbox: "ready" } : {}) },
    providerUsage: reading(provider),
  }));
  const cssRoot = resolve(import.meta.dirname, "../out/_next/static/chunks");
  const css = readdirSync(cssRoot).filter((file) => file.endsWith(".css")).map((file) => readFileSync(join(cssRoot, file), "utf8")).join("\n");
  const screenshots = resolve(import.meta.dirname, "../../../docs/screenshots/phone");
  mkdirSync(screenshots, { recursive: true });
  const browser = await chromium.launch({ channel: "chrome", headless: true });
  try {
    const page = await browser.newPage();
    const show = async (light = false): Promise<void> => {
      const html = renderToStaticMarkup(createElement(ProviderUsage, { accounts, now, busy: null, problem: null, onRefresh: () => {} }));
      await page.setContent(`<!doctype html><html class="${light ? "" : "dark"}"><head><meta name="viewport" content="width=device-width"><style>${css}</style></head><body><main style="padding:20px 16px"><h1 style="font-size:20px;font-weight:600">Usage</h1>${html}<p style="margin-top:16px;font-size:12px;color:var(--secondary)">Synthetic provider readings · layout verification only</p></main></body></html>`);
    };
    for (const width of [390, 360, 650]) {
      await page.setViewportSize({ width, height: width === 360 ? 640 : 844 });
      for (const light of [false, true]) {
        await show(light);
        expect(await page.evaluate("document.documentElement.scrollWidth <= innerWidth")).toBe(true);
        expect(await page.getByText("38,917.83 left", { exact: true }).isVisible()).toBe(true);
        expect(await page.getByRole("meter", { name: "Weekly · Fable", exact: true }).getAttribute("aria-valuenow")).toBeNull();
        await page.screenshot({ path: join(screenshots, `provider-cards-${width}-${light ? "light" : "dark"}.png`), fullPage: true });
      }
    }
    const claude = accounts[0].providerUsage;
    if (!claude) throw new Error("Fixture reading missing");
    accounts[0].providerUsage = { ...claude, status: "error", checkedAt: "2026-10-22T13:00:00.000Z", error: "Couldn't refresh this provider." };
    await show();
    expect(await page.getByText(/Last updated/).isVisible()).toBe(true);
    expect(await page.getByText("8%", { exact: false }).count()).toBeGreaterThan(0);
    expect(await page.getByText("Couldn't refresh this provider.", { exact: true }).isVisible()).toBe(true);
  } finally { await browser.close(); }
}, 60_000);
