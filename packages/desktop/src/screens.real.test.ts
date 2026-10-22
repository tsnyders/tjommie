import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { botReply, claudeBot, closeRealApp, launchRealApp, openChat, send, type RealApp } from "./real-app.ts";

// The screens from plan 1E, used with real Claude bots. These tests run in order and build on each other.
let app: RealApp | undefined;

beforeAll(async () => {
  app = await launchRealApp();
});

afterAll(async () => {
  await closeRealApp(app, false);
});

function running(): RealApp {
  if (!app) throw new Error("The desktop app didn't start.");
  return app;
}

describe("the Phase 1 screens with real bots", () => {
  it("shows first-run setup with both providers signed in", async () => {
    const { page } = running();
    await expect.poll(() => page.getByText(/^Signed in/).count(), { timeout: 120_000 }).toBe(2);
  });

  it("creates a bot from a suggestion, with its routine, and chats with it", async () => {
    const { page } = running();
    await page.getByRole("link", { name: "Create your first bot" }).click();
    await page.getByRole("button", { name: /^Job hunter/ }).click();
    await expect.poll(() => page.getByLabel("Name").inputValue()).toBe("Job hunter");
    await page.getByRole("button", { name: /^Model/ }).click();
    await page.getByRole("button", { name: "Haiku (latest)" }).click();
    await page.getByRole("button", { name: "Create bot" }).click();
    await page.getByRole("heading", { level: 1, name: "Job hunter", exact: true }).waitFor();
    await page.getByRole("button", { name: /^Job search \(3h\)/ }).waitFor();
    await send(page, "Job hunter", "Reply with exactly: PONG");
    await botReply(page, "PONG").waitFor({ timeout: 180_000 });
  });

  it("adds, edits and deletes a routine from the bot panel", async () => {
    const { page } = running();
    await page.getByRole("button", { name: "Add a routine" }).click();
    const create = page.getByRole("dialog", { name: "New routine" });
    await create.getByLabel("Name").fill("Morning check");
    await create.getByLabel("What to do").fill("Check the news and tell me one headline.");
    await create.getByRole("button", { name: "Save" }).click();
    await page.getByRole("button", { name: /^Morning check Weekdays at 9:00 AM/ }).click();
    const edit = page.getByRole("dialog", { name: "Edit routine" });
    await edit.getByRole("radio", { name: "1 hour" }).click();
    await edit.getByRole("button", { name: "Save" }).click();
    await page.getByRole("button", { name: /^Morning check \(1h\)/ }).click();
    await page.getByRole("dialog", { name: "Edit routine" }).getByRole("button", { name: "Delete routine" }).click();
    await page.getByRole("button", { name: /^Morning check/ }).waitFor({ state: "detached" });
  });

  it("renames a bot in Bot settings and shows its notes", async () => {
    const { page } = running();
    await page.getByRole("link", { name: "Bot settings", exact: true }).click();
    await page.getByText("# Job hunter's notes").waitFor();
    await page.getByLabel("Name").fill("Job scout");
    await page.getByRole("button", { name: "Save changes" }).click();
    await page.getByRole("heading", { level: 1, name: "Job scout", exact: true }).waitFor();
  });

  it("answers a mention in the Team room, and hints when nobody was mentioned", async () => {
    const { page } = running();
    await page.getByRole("link", { name: "Team room", exact: true }).click();
    await page.getByText("No posts yet. @mention a bot to ask it something.").waitFor();
    const box = page.getByRole("combobox", { name: "Message the Team room" });
    await box.fill("Hello team");
    await box.press("Enter");
    await page.getByText("No bot was mentioned, so nobody will reply. Type @ to ask a bot.").waitFor();
    await box.fill("@Jo");
    await page.getByRole("option", { name: /Job scout/ }).click();
    await box.pressSequentially("reply with exactly: ROOM-OK");
    await box.press("Enter");
    await page.locator('[data-author="you"]', { hasText: "@Job scout reply with exactly: ROOM-OK" }).waitFor();
    await botReply(page, "ROOM-OK").waitFor({ timeout: 180_000 });
  });

  it("opens an exchange from a handoff row and replies in the Team room", async () => {
    const { client, page } = running();
    const writer = await claudeBot(client, "Writer");
    await claudeBot(client, "Researcher", { canMessage: [writer.id] });
    await openChat(page, "Researcher");
    await send(page, "Researcher", "Use the message_bot tool to ask Writer to reply with exactly the word MANGO. When Writer's answer arrives, tell me what Writer said.");
    const row = page.getByRole("button", { name: /Researcher asked Writer/ });
    await row.getByText(/messages?$/).waitFor({ timeout: 240_000 });
    await row.click();
    const view = page.getByRole("complementary", { name: "Exchange" });
    await view.getByText(/Replied after/).waitFor({ timeout: 30_000 });
    // Researcher's request says MANGO too, so the check is on the last bubble, Writer's reply ("MANGO." passes).
    await view.locator('[data-author="bot"]').last().filter({ hasText: "MANGO" }).waitFor();
    await view.getByRole("textbox", { name: "Reply in Team room" }).fill("Thanks, both of you.");
    await view.getByRole("button", { name: "Send" }).click();
    await view.getByText("Posted in the Team room.").waitFor();
  });

  it("shows every bot on the Agents overview, and filters them", async () => {
    const { page } = running();
    await page.getByRole("link", { name: "Agents", exact: true }).click();
    await page.getByRole("heading", { level: 1, name: "Agents", exact: true }).waitFor();
    for (const name of ["Job scout", "Writer", "Researcher"]) await page.getByRole("link", { name, exact: true }).waitFor();
    await page.getByRole("button", { name: "Menu for Writer" }).click();
    await page.getByRole("menuitem", { name: "Pause bot" }).click();
    await page.getByRole("radio", { name: "Paused" }).click();
    await page.getByRole("link", { name: "Writer", exact: true }).waitFor();
    await page.getByRole("link", { name: "Job scout", exact: true }).waitFor({ state: "detached" });
    await page.getByRole("button", { name: "Menu for Writer" }).click();
    await page.getByRole("menuitem", { name: "Resume bot" }).click();
    await page.getByRole("radio", { name: "All" }).click();
    await page.getByRole("link", { name: "Job scout", exact: true }).waitFor();
    // Writer may still be answering the Team room post from the exchange test before it settles to Idle.
    const writer = page.getByRole("article").filter({ has: page.getByRole("link", { name: "Writer", exact: true }) });
    await writer.getByText("Idle", { exact: true }).first().waitFor({ timeout: 180_000 });
  });

  it("switches the theme and keeps the chain limit", async () => {
    const { client, page } = running();
    await page.getByRole("link", { name: "Settings", exact: true }).click();
    await page.getByRole("radio", { name: "Light" }).click();
    await expect.poll(async () => (await page.locator("html").getAttribute("class")) ?? "").not.toContain("dark");
    await page.getByRole("radio", { name: "Dark" }).click();
    await expect.poll(async () => (await page.locator("html").getAttribute("class")) ?? "").toContain("dark");
    await page.getByRole("link", { name: "Bots", exact: true }).click();
    await page.getByRole("button", { name: "Increase" }).click();
    await expect.poll(async () => (await client.call("settings.get", {})).chainLimit).toBe(6);
  });

  it("deletes a bot after confirming", async () => {
    const { client, page } = running();
    await openChat(page, "Writer");
    await page.getByRole("link", { name: "Bot settings", exact: true }).click();
    await page.getByRole("button", { name: "Delete bot" }).click();
    await page.getByRole("dialog", { name: "Delete Writer?" }).getByRole("button", { name: "Delete bot" }).click();
    await expect.poll(async () => (await client.call("bots.list", {})).some((bot) => bot.name === "Writer"), { timeout: 60_000 }).toBe(false);
    // The UI leaves the deleted bot's page (for the first bot's chat) and drops it from the rail.
    await page.getByRole("heading", { level: 1, name: "Writer", exact: true }).waitFor({ state: "detached" });
    await page.getByRole("heading", { level: 1, name: "Job scout", exact: true }).waitFor();
    await page.getByRole("link", { name: /^Writer,/ }).waitFor({ state: "detached" });
  });
});
