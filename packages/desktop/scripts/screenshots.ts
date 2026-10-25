// Screenshots of every Phase 1 screen, from the running app with real bots: nothing is faked. Build the UI first:
// npm run build, then node --disable-warning=ExperimentalWarning packages/desktop/scripts/screenshots.ts
import { mkdirSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import type { Page } from "playwright-core";
import { botReply, claudeBot, closeRealApp, eventually, launchRealApp, openChat, send } from "../src/real-app.ts";

const out = join(resolve(import.meta.dirname, "..", "..", ".."), "docs", "screenshots", "phase1");
mkdirSync(out, { recursive: true });

async function theme(page: Page, choice: "Light" | "Dark"): Promise<void> {
  await page.getByRole("link", { name: "Settings", exact: true }).click();
  await page.getByRole("radio", { name: choice }).click();
}

const hhmm = (date: Date): string => `${String(date.getHours()).padStart(2, "0")}:${String(date.getMinutes()).padStart(2, "0")}`;

const app = await launchRealApp();
try {
  const { electronApp, page, client } = app;
  await electronApp.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows()[0]?.setContentSize(1440, 900)); // the frames' size

  // The whole window, as Windows draws it: page.screenshot would leave out the caption buttons the frames show.
  const shot = async (name: string): Promise<void> => {
    await new Promise((done) => setTimeout(done, 500)); // dialogs and popovers finish opening
    const png = await electronApp.evaluate(async ({ BrowserWindow, desktopCapturer }) => {
      const window = BrowserWindow.getAllWindows()[0];
      const [width, height] = window.getContentSize();
      const sources = await desktopCapturer.getSources({ types: ["window"], thumbnailSize: { width, height } });
      return sources.find((source) => source.id === window.getMediaSourceId())?.thumbnail.toPNG().toString("base64");
    });
    if (!png) throw new Error(`The window couldn't be captured for ${name}.`);
    writeFileSync(join(out, `${name}.png`), Buffer.from(png, "base64"));
  };

  // WF9: first run, once both providers are checked.
  await eventually(async () => ((await page.getByText(/^Signed in/).count()) === 2 ? true : undefined), 120_000);
  await shot("wf9-first-run-dark");

  // WF10: Settings.
  await page.getByRole("link", { name: "Settings", exact: true }).click();
  await page.getByRole("heading", { name: "General", exact: true }).waitFor();
  await shot("wf10-settings-general-dark");
  await page.getByRole("link", { name: "Bots", exact: true }).click();
  await page.getByText("Requests between bots per chain").waitFor();
  await shot("wf10-settings-bots-dark");

  // Bots like WF1's. CV Writer is made through the API to save time; Pnet apply the way a person would, with WF3's
  // instructions and Can message closed again, as WF3 draws them.
  const writer = await claudeBot(client, "CV Writer", { color: "green", shape: "rounded-square", instructions: "You tailor CVs. Answer in one short line." });
  await page.getByRole("link", { name: "New bot", exact: true }).click();
  await page.getByRole("button", { name: /^Job hunter/ }).click();
  await page.getByLabel("Name").fill("Pnet apply");
  await page.getByLabel("What should this bot do?").fill("Apply to Full Stack and React roles on pnet.co.za in Johannesburg or remote. Skip senior roles. Ask CV Writer for a tailored CV when a listing asks for Angular or Azure.");
  await page.getByRole("button", { name: /^Can message/ }).click();
  await page.getByRole("switch", { name: "Can message CV Writer" }).click();
  await page.getByRole("button", { name: /^Can message/ }).click();
  await shot("wf3-new-bot-dark"); // WF3
  await page.getByRole("button", { name: /^Model/ }).click();
  await page.getByRole("button", { name: "Haiku (latest)" }).click();
  await page.getByRole("radio", { name: "Always ask" }).click();
  await page.getByRole("button", { name: "Create bot" }).click();
  await page.getByRole("heading", { level: 1, name: "Pnet apply", exact: true }).waitFor();

  // WF1 and WF6: an exchange with a shared file, then an approval card left waiting. Pnet apply passes the request on
  // word for word, so CV Writer is the one who shares the file. The bot may ask CV Writer more than once; the first
  // exchange is the one shown.
  const exchange = page.getByRole("button", { name: /Pnet apply asked CV Writer/ }).first();
  await send(page, "Pnet apply", "Use the message_bot tool to send CV Writer exactly this message: 'Write a file called cv-note.txt containing the word hello in your own folder, share it with the share_file tool, then reply DONE.' When CV Writer answers, tell me in one line.");
  await exchange.getByText(/messages?$/).waitFor({ timeout: 300_000 });
  await send(page, "Pnet apply", "Use the Write tool to create notes-test.txt containing hi. Then reply DONE.");
  await page.getByRole("group", { name: "Waiting for you" }).first().waitFor({ timeout: 180_000 });
  await shot("wf1-main-chat-dark");
  await theme(page, "Light");
  await openChat(page, "Pnet apply");
  await shot("wf1-main-chat-light");
  await theme(page, "Dark");
  await openChat(page, "Pnet apply");
  await exchange.click();
  await page.getByRole("complementary", { name: "Exchange" }).getByText(/Replied after/).waitFor();
  await shot("wf6-exchange-view-dark");
  await page.getByRole("button", { name: "Close the exchange" }).click();

  // WF7: a routine that has really run three times, two minutes apart, starting once Pnet apply is free: Done; Stopped
  // by you while it waited to write a file; Skipped while the bot was paused.
  await page.getByRole("group", { name: "Waiting for you" }).first().getByRole("button", { name: "Allow once" }).click();
  await page.getByText(/^Allowed once: /).waitFor();
  const pnet = (await client.call("bots.list", {})).find((bot) => bot.name === "Pnet apply");
  if (!pnet) throw new Error("Pnet apply wasn't created.");
  const idle = async () => ((await client.call("bots.list", {})).find((bot) => bot.id === pnet.id)?.state === "idle" ? true : undefined);
  // It can go on to more steps after the card. A routine run waits for those, so one still going after three minutes
  // is stopped, as a person would.
  await eventually(idle, 180_000).catch(async () => {
    await page.getByRole("button", { name: "Stop", exact: true }).click();
    await eventually(idle, 60_000);
  });
  const first = Math.ceil((Date.now() + 90_000) / 60_000) * 60_000;
  const due = [0, 2, 4].map((minutes) => new Date(first + minutes * 60_000));
  const quick = "Reply with exactly: ROUTINE-OK";
  const routine = await client.call("routines.create", { botId: pnet.id, name: "Pnet apply 9am", prompt: quick, days: [...new Set(due.map((date) => date.getDay()))], times: due.map(hhmm), timeLimitMinutes: 180 });
  const ran = (status: string) => eventually(async () => ((await client.call("routines.runs", { routineId: routine.id })).some((run) => run.status === status) ? true : undefined), 600_000);
  await ran("done");
  await client.call("routines.update", { id: routine.id, patch: { prompt: "Use the Write tool to create routine-note.txt containing hi. Then reply DONE." } });
  await page.getByRole("group", { name: "Waiting for you" }).first().waitFor({ timeout: 600_000 });
  await page.getByRole("button", { name: "Stop", exact: true }).click();
  await ran("stopped");
  await client.call("routines.update", { id: routine.id, patch: { prompt: quick } });
  await client.call("bots.pause", { id: pnet.id, paused: true });
  await ran("skipped");
  await client.call("bots.pause", { id: pnet.id, paused: false });
  await page.getByRole("button", { name: /^Pnet apply 9am/ }).click();
  const editor = page.getByRole("dialog", { name: "Edit routine" });
  for (const text of ["Done", "Stopped by you", "Skipped: Pnet apply was paused"]) await editor.getByText(text, { exact: true }).waitFor();
  await shot("wf7-routine-editor-dark");
  await page.keyboard.press("Escape");

  // WF8, and WF11's dialogs and model picker. From here on Pnet apply waits for you, so its chip, the members, the
  // popover and WF2's cards show that state.
  await send(page, "Pnet apply", "Use the Write tool to create agents-note.txt containing hi. Then reply DONE.");
  await page.getByRole("group", { name: "Waiting for you" }).first().waitFor({ timeout: 180_000 });
  await page.getByRole("link", { name: "Bot settings", exact: true }).click();
  await page.getByText("# Pnet apply's notes").waitFor();
  await shot("wf8-bot-settings-dark");
  await page.getByRole("radio", { name: "Full auto" }).click();
  const fullAuto = page.getByRole("dialog", { name: "Let Pnet apply work without asking?" });
  await fullAuto.waitFor();
  await shot("wf11-full-auto-dark");
  await fullAuto.getByRole("button", { name: "Cancel" }).click();
  await page.getByRole("button", { name: "Delete bot" }).click();
  const remove = page.getByRole("dialog", { name: "Delete Pnet apply?" });
  await remove.waitFor();
  await shot("wf11-delete-bot-dark");
  await remove.getByRole("button", { name: "Cancel" }).click();
  await page.getByRole("button", { name: /^Model/ }).click();
  await page.getByRole("button", { name: "Haiku (latest)", exact: true }).waitFor(); // the Model row names the model too
  await shot("wf11-model-picker-dark");
  await page.keyboard.press("Escape");

  // WF5: an answered mention. Then CV Writer is paused too, and the popover is open over a new post.
  await page.getByRole("link", { name: "Rooms", exact: true }).click();
  await page.getByRole("link", { name: "Everyone", exact: true }).click();
  const box = page.getByRole("combobox", { name: "Message Everyone" });
  await box.fill("@CV Writer reply with exactly: ROOM-OK");
  await box.press("Enter");
  await botReply(page, "ROOM-OK").waitFor({ timeout: 180_000 });
  await client.call("bots.pause", { id: writer.id, paused: true });
  await box.fill("Can someone check careers24.com for new React roles? @C");
  await page.getByRole("option").first().waitFor();
  await shot("wf5-team-room-dark");
  await box.fill("");

  // WF2, with the pointer on CV Writer's card, as WF2 draws a hovered card.
  await page.getByRole("link", { name: "Agents", exact: true }).click();
  await page.getByRole("heading", { level: 1, name: "Agents", exact: true }).waitFor();
  await page.getByText("Review", { exact: true }).waitFor();
  await page.getByText(/ (talked|answered) /).waitFor(); // Everyone's card line
  await page.getByRole("link", { name: "CV Writer", exact: true }).hover();
  await shot("wf2-agents-overview-dark");
} finally {
  await closeRealApp(app, false);
}
