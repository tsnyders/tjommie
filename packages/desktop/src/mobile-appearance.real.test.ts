import { mkdirSync } from "node:fs";
import { resolve } from "node:path";
import { expect, it } from "vitest";
import { closeRealApp, eventually, launchRealApp, type RealApp } from "./real-app.ts";

const styles = ["hush", "folio", "aurora"] as const;
const canvases = {
  hush: { dark: "rgb(7, 7, 7)", light: "rgb(252, 252, 252)" },
  folio: { dark: "rgb(18, 17, 14)", light: "rgb(243, 239, 230)" },
  aurora: { dark: "rgb(5, 6, 10)", light: "rgb(238, 241, 247)" },
};

it("persists three phone styles, keeps floating navigation and menus reachable, and leaves desktop colors unchanged", async () => {
  let app: RealApp | undefined;
  const errors: string[] = [];
  mkdirSync(resolve("docs/screenshots/phone/appearance"), { recursive: true });
  try {
    app = await launchRealApp(undefined, { isolatedAccounts: true });
    const { page, client } = app;
    page.setDefaultTimeout(15_000);
    page.setDefaultNavigationTimeout(20_000);
    page.on("pageerror", (error) => errors.push(error.message));
    const bot = await client.call("bots.create", { name: "Appearance fixture", provider: "claude", model: "haiku", color: "pink", shape: "cloud", instructions: "Synthetic unsigned appearance fixture", autonomy: "ask", browser: false });
    await client.call("bots.pause", { id: bot.id, paused: true });
    await app.electronApp.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows()[0].setContentSize(411, 839));
    await eventually(async () => await page.evaluate("innerWidth") === 411 || undefined);
    const origin = page.url();
    // Use the same persisted values as the settings controls; each reload also
    // exercises the before-paint theme/style initialization, without auth calls.
    for (const width of [411, 360]) {
      await app.electronApp.evaluate(({ BrowserWindow }, w) => BrowserWindow.getAllWindows()[0].setContentSize(w, 839), width);
      await eventually(async () => await page.evaluate("innerWidth") === width || undefined);
      for (const style of styles) {
        for (const theme of ["dark", "light"] as const) {
          await page.evaluate(({ style, theme }) => { localStorage.setItem("tjommie.mobileStyle", style); localStorage.setItem("tjommie.theme", theme); }, { style, theme });
          await page.goto(new URL("/agents/", origin).href);
          await page.getByRole("heading", { name: "Agents", exact: true }).waitFor();
          expect(await page.locator("html").getAttribute("data-mobile-style")).toBe(style);
          expect(await page.evaluate(style === "aurora" ? "getComputedStyle(document.documentElement).backgroundColor" : "getComputedStyle(document.body).backgroundColor")).toBe(canvases[style][theme]);
          expect(await page.evaluate("document.documentElement.scrollWidth<=innerWidth")).toBe(true);
          const menuButton = page.getByRole("button", { name: `Menu for ${bot.name}`, exact: true });
          await eventually(async () => { const rect = await menuButton.boundingBox(); return rect && rect.width >= 43.99 && rect.height >= 43.99 || undefined; });
          const menu = await menuButton.boundingBox();
          expect(menu!.width).toBeGreaterThanOrEqual(43.99);
          expect(menu!.height).toBeGreaterThanOrEqual(43.99);
          expect(await page.evaluate("(()=>{const row=document.querySelector('[data-slot=bot-row]');const name=row.querySelector('[data-slot=row-name]').getBoundingClientRect();const state=row.querySelector('[data-slot=row-chip]').getBoundingClientRect();return name.right<=state.left+1;})()")).toBe(true);
          const nav = page.getByRole("navigation", { name: "Tjommie", exact: true });
          const box = await nav.boundingBox();
          expect(box).not.toBeNull();
          expect(box!.width).toBeLessThan(width - 50);
          expect(box!.x).toBeGreaterThan(20);
          expect(box!.y + box!.height).toBeLessThanOrEqual(839);
          await page.screenshot({ path: resolve(`docs/screenshots/phone/appearance/${style}-${theme}-${width}-agents.png`), animations: "disabled" });
          // The rooms list draws its rows as the Agents rows are drawn, in each style.
          await page.goto(new URL("/room/", origin).href);
          await page.getByRole("heading", { name: "Rooms", exact: true }).waitFor();
          await page.getByRole("link", { name: "Everyone", exact: true }).waitFor();
          expect(await page.evaluate("document.documentElement.scrollWidth<=innerWidth")).toBe(true);
          await page.screenshot({ path: resolve(`docs/screenshots/phone/appearance/${style}-${theme}-${width}-rooms.png`), animations: "disabled" });
          await page.goto(new URL("/room/?id=team", origin).href);
          await page.getByRole("heading", { name: /^Everyone/ }).waitFor();
          const roomBox = await page.getByRole("combobox", { name: "Message Everyone", exact: true }).boundingBox();
          const roomNav = await nav.boundingBox();
          expect(roomBox!.y + roomBox!.height).toBeLessThan(roomNav!.y);
          expect(await page.evaluate("document.documentElement.scrollWidth<=innerWidth")).toBe(true);
          await page.getByRole("heading", { name: /^Everyone/ }).getByRole("button").click();
          await page.getByRole("complementary", { name: "Everyone members", exact: true }).waitFor();
          await eventually(async () => await page.getByTestId("phone-nav").getAttribute("aria-hidden") === "true" || undefined, 5000);
          await page.getByRole("button", { name: "Hide the members", exact: true }).click();
          await nav.waitFor();
          await page.goto(new URL(`/chat/?bot=${bot.id}`, origin).href);
          await page.getByRole("button", { name: /Show details/ }).click();
          await page.getByRole("button", { name: "Add a routine", exact: true }).waitFor();
          await page.getByRole("button", { name: "Close details", exact: true }).click();
          expect(await nav.count()).toBe(0);
          await page.goto(new URL("/settings/?section=usage", origin).href);
          const ranges = page.getByRole("group", { name: "Range", exact: true });
          await ranges.waitFor();
          expect(await ranges.evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);
          for (const chip of await ranges.getByRole("button").all()) {
            const bounds = await chip.boundingBox();
            expect(bounds!.height).toBeGreaterThanOrEqual(43.99);
            expect(bounds!.x + bounds!.width).toBeLessThanOrEqual(width);
          }
        }
      }
    }
    await app.electronApp.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows()[0].setContentSize(1100, 800));
    await eventually(async () => await page.evaluate("innerWidth") === 1100 || undefined);
    for (const style of styles) {
      await page.evaluate((style) => { localStorage.setItem("tjommie.mobileStyle", style); localStorage.setItem("tjommie.theme", "dark"); }, style);
      await page.goto(new URL("/agents/", origin).href);
      await page.getByRole("heading", { name: "Agents", exact: true }).waitFor();
      expect(await page.evaluate("getComputedStyle(document.body).backgroundColor")).toBe("rgb(7, 7, 7)");
    }
    expect(errors).toEqual([]);
  } finally { await closeRealApp(app, false); }
}, 180_000);
