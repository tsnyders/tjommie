import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import {
  botReply, claudeBot, closeRealApp, daemonConnection, eventually, launchRealApp,
  openChat, quitApp, refused, rememberDaemon, send, terminateOwnedProcess,
  type RealApp,
} from "./real-app.ts";

// The desktop chat and lifecycle, exercised through the production Electron app and real Claude bots.
let app: RealApp | undefined;
let quit = false;
const next404s: string[] = [];
let nextResponseCount = 0;

beforeAll(async () => {
  app = await launchRealApp();
  // The first window can load before this listener attaches; the rail navigation below supplies the check.
  app.page.on("response", (response) => {
    if (!response.url().startsWith("app://tjommie/")) return;
    const url = new URL(response.url());
    if (url.pathname.split("/").some((part) => part.startsWith("__next."))) {
      nextResponseCount++;
      if (response.status() === 404) next404s.push(url.pathname);
    }
  });
});

afterAll(async () => {
  await closeRealApp(app, quit);
});

function running(): RealApp {
  if (!app) throw new Error("The desktop app didn't start.");
  return app;
}

async function openChatChecked(name: string): Promise<void> {
  await openChat(running().page, name);
  expect(next404s).toEqual([]);
}

function tokenAbsentFromLog(token: string): void {
  const logFile = join(running().root, "logs", "desktop.log");
  expect(existsSync(logFile)).toBe(true);
  // A boolean assertion cannot print the token or log contents.
  expect(readFileSync(logFile, "utf8").includes(token)).toBe(false);
}

describe("the desktop app with real bots", () => {
  it("sends a message and shows the reply", async () => {
    const { client, page } = running();
    await claudeBot(client, "Echo");
    await openChatChecked("Echo");
    expect(nextResponseCount).toBeGreaterThan(0);
    await send(page, "Echo", "Reply with exactly: PONG");
    await botReply(page, "PONG").waitFor({ timeout: 180_000 });
  });

  it("asks before a change and carries on after Allow once", async () => {
    const { client, page } = running();
    const bot = await claudeBot(client, "Careful", { autonomy: "ask" });
    await openChatChecked("Careful");
    await send(page, "Careful", "Use the Write tool to create hello.txt containing the word hi. Then reply DONE.");
    const card = page.getByRole("group", { name: "Waiting for you" });
    await card.waitFor({ timeout: 180_000 });
    await card.getByRole("button", { name: "Allow once" }).click();
    await page.getByText(/^Allowed once: /).waitFor();
    await botReply(page, "DONE").waitFor({ timeout: 180_000 });
    expect(existsSync(join(bot.homeFolder, "hello.txt"))).toBe(true);
  });

  it("stops a bot that's working", async () => {
    const { client, page } = running();
    await claudeBot(client, "Sleeper", { autonomy: "full" });
    await openChatChecked("Sleeper");
    await send(page, "Sleeper", `Use the Bash tool, not in the background, to run exactly: node -e "setTimeout(() => console.log('done'), 60000)". Then tell me what it printed.`);
    await page.getByRole("button", { name: /^\d+ steps?: / }).waitFor({ timeout: 180_000 });
    await page.getByRole("button", { name: "Stop" }).click();
    await page.getByText("Stopped.", { exact: true }).waitFor({ timeout: 30_000 });
    await page.getByRole("link", { name: "Sleeper, Idle" }).waitFor({ timeout: 30_000 });
  });

  it("reconnects after the daemon dies", async () => {
    const { page } = running();
    const before = await daemonConnection(page);
    const pid = rememberDaemon(running(), before.port);
    await terminateOwnedProcess(running(), pid);
    // Electron gives the UI a new connection with a new token after restarting the daemon.
    const after = await eventually(async () => {
      const next = await daemonConnection(page);
      return next.token === before.token ? undefined : next;
    });
    const nextPid = rememberDaemon(running(), after.port);
    expect(nextPid).not.toBe(pid);
    tokenAbsentFromLog(before.token);
    tokenAbsentFromLog(after.token);
    await page.getByText("Reconnecting to Tjommie").waitFor({ state: "hidden", timeout: 60_000 });
    await openChatChecked("Echo");
    await page.getByRole("button", { name: "More" }).click();
    await page.getByRole("menuitem", { name: "Pause bot" }).click();
    await page.getByRole("link", { name: "Echo, Paused" }).waitFor();
    await page.getByRole("button", { name: "More" }).click();
    await page.getByRole("menuitem", { name: "Resume bot" }).click();
    await page.getByRole("link", { name: "Echo, Idle" }).waitFor();
  });

  it("quitting Tjommie stops its daemon", async () => {
    const { page } = running();
    const { port, token } = await daemonConnection(page);
    rememberDaemon(running(), port);
    tokenAbsentFromLog(token);
    await quitApp(running());
    quit = true;
    expect(await refused(port)).toBe(true);
    tokenAbsentFromLog(token);
    expect(next404s).toEqual([]);
  });
});
