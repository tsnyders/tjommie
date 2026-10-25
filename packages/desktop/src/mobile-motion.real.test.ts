import { expect, it } from "vitest";
import { closeRealApp, eventually, launchRealApp, type RealApp } from "./real-app.ts";

it("animates phone surfaces within their bounds, respects reduced motion and leaves desktop motion unchanged", async () => {
  let app: RealApp | undefined;
  try {
    app = await launchRealApp(undefined, { isolatedAccounts: true });
    const { page, client } = app;
    const bot = await client.call("bots.create", { name: "Mobile motion fixture", provider: "claude", model: "haiku", color: "blue", shape: "circle", instructions: "Synthetic motion fixture", autonomy: "ask", browser: false });
    await client.call("bots.pause", { id: bot.id, paused: true });
    await app.electronApp.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows()[0].setContentSize(360, 640));
    await page.emulateMedia({ reducedMotion: "no-preference" });
    await page.goto(new URL(`/chat/?bot=${bot.id}`, page.url()).href);
    await page.getByRole("button", { name: /Show details/ }).click();
    const panel = page.getByRole("complementary", { name: `About ${bot.name}`, exact: true });
    await panel.waitFor();
    expect(await panel.evaluate((el) => el.ownerDocument.defaultView!.getComputedStyle(el).animationName)).toBe("phone-sheet-in");
    // Freeze halfway through a real browser animation, checking both interpolation and bounds.
    const middle = await page.evaluate<{ opacity: number; inside: boolean; scrolls: boolean }>("(()=>{const el=document.querySelector('aside.mobile-motion-sheet');el.style.animation='none';void el.offsetHeight;el.style.animation='';const animation=el.getAnimations()[0];animation.pause();animation.currentTime=100;const box=el.getBoundingClientRect();const result={opacity:Number(getComputedStyle(el).opacity),inside:box.left>=0&&box.top>=0&&box.right<=innerWidth&&box.bottom<=innerHeight,scrolls:document.documentElement.scrollWidth>innerWidth};animation.play();return result;})()");
    expect(middle.opacity).toBeGreaterThan(0); expect(middle.opacity).toBeLessThan(1);
    expect(middle.inside).toBe(true); expect(middle.scrolls).toBe(false);
    await page.getByRole("button", { name: "Close details", exact: true }).click();
    await page.getByRole("button", { name: "More", exact: true }).click();
    let menu = page.getByRole("menu");
    await menu.waitFor();
    expect(await menu.evaluate((el) => el.ownerDocument.defaultView!.getComputedStyle(el).transitionDuration)).toContain("0.16s");
    await page.keyboard.press("Escape");
    await menu.waitFor({ state: "hidden" });

    await page.emulateMedia({ reducedMotion: "reduce" });
    await page.getByRole("button", { name: /Show details/ }).click();
    expect(await panel.evaluate((el) => el.ownerDocument.defaultView!.getComputedStyle(el).animationName)).toBe("none");
    await page.getByRole("button", { name: "Add a routine", exact: true }).click();
    const dialog = page.getByRole("dialog", { name: "New routine", exact: true });
    await dialog.waitFor();
    expect(await dialog.evaluate((el) => el.ownerDocument.defaultView!.getComputedStyle(el).transitionProperty)).toBe("none");
    await dialog.getByRole("button", { name: "Cancel", exact: true }).click();
    await page.getByRole("button", { name: "Close details", exact: true }).click();

    await page.emulateMedia({ reducedMotion: "no-preference" });
    await app.electronApp.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows()[0].setContentSize(1100, 800));
    await eventually(async () => await page.evaluate("innerWidth") === 1100 || undefined);
    await page.getByRole("button", { name: "More", exact: true }).click();
    menu = page.getByRole("menu");
    await menu.waitFor();
    expect(await menu.evaluate((el) => el.ownerDocument.defaultView!.getComputedStyle(el).transitionDuration)).toBe("0s");
    expect(await panel.evaluate((el) => el.ownerDocument.defaultView!.getComputedStyle(el).animationName)).toBe("none");
    await page.keyboard.press("Escape");
  } finally { await closeRealApp(app, false); }
}, 90_000);
