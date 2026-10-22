import { createServer, type Server } from "node:http";
import { execFileSync } from "node:child_process";
import { join, resolve } from "node:path";
import { mkdirSync, writeFileSync } from "node:fs";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { closeRealApp, launchRealApp, openChat, type RealApp } from "./real-app.ts";

let app: RealApp | undefined;
let site: Server;
let origin: string;
let signedInAs: string | null = null;
let typedValue = "";
const inputEvents: Record<string, string>[] = [];
let sawCookie = false;

beforeAll(async () => {
  site = createServer((req, res) => {
    const url = new URL(req.url ?? "/", "http://fixture.invalid");
    if (url.pathname === "/input") { typedValue = url.searchParams.get("value") ?? ""; res.writeHead(204).end(); return; }
    if (url.pathname === "/event") { inputEvents.push(Object.fromEntries(url.searchParams)); res.writeHead(204).end(); return; }
    if (url.pathname === "/login") {
      signedInAs = url.searchParams.get("name");
      res.writeHead(302, { "set-cookie": "fixture_session=local-only; Path=/; SameSite=Lax", location: "/signed-in" }).end();
      return;
    }
    if (url.pathname === "/signed-in") sawCookie = req.headers.cookie?.includes("fixture_session=local-only") ?? false;
    res.writeHead(200, { "content-type": "text/html; charset=utf-8" }).end(`<!doctype html><html><head><title>${url.pathname === "/popup" ? "Fixture popup" : "Fixture browser"}</title></head>
      <body style="margin:0;background:white;color:black;font:16px sans-serif">
      <form action="/login"><input aria-label="Fixture name" name="name" oninput="fetch('/input?value='+encodeURIComponent(this.value))" style="position:absolute;left:20px;top:20px;width:200px;height:35px"><button style="position:absolute;left:20px;top:75px;height:35px">Sign in</button></form>
      <button onclick="window.open('/popup')" style="position:absolute;left:20px;top:130px;height:35px">Open popup</button>
      <button onclick="confirm('Fixture confirmation')" style="position:absolute;left:20px;top:185px;height:35px">Show confirmation</button>
      <p style="position:absolute;left:20px;top:240px">${url.pathname}</p><script>
      addEventListener('pointerdown', e => fetch('/event?kind=pointer&x='+e.clientX+'&y='+e.clientY+'&width='+innerWidth+'&height='+innerHeight+'&target='+e.target.tagName));
      addEventListener('keydown', e => fetch('/event?kind=key&target='+e.target.tagName));
      </script></body></html>`);
  });
  await new Promise<void>((resolve) => site.listen(0, "127.0.0.1", resolve));
  const address = site.address();
  if (!address || typeof address === "string") throw new Error("Fixture site did not bind a port.");
  origin = `http://127.0.0.1:${address.port}`;
  // Browser acceptance does not need a provider login or touch the person's managed accounts.
  app = await launchRealApp(undefined, { isolatedAccounts: true });
});

afterAll(async () => {
  if (app) {
    const cache = resolve(import.meta.dirname, "../../../node_modules/.cache");
    mkdirSync(cache, { recursive: true });
    await app.page.screenshot({ path: join(cache, "embedded-browser-debug.png") }).catch(() => {});
    const probe = await app.page.evaluate(() => (globalThis as unknown as { browserProbe?: unknown[] }).browserProbe ?? []).catch(() => []);
    writeFileSync(join(cache, "embedded-browser-probe.json"), JSON.stringify(probe, null, 2));
  }
  try { await closeRealApp(app, false); }
  finally { if (site) await new Promise<void>((resolve, reject) => site.close((error) => error ? reject(error) : resolve())); }
});

function running(): RealApp {
  if (!app) throw new Error("The app did not start.");
  return app;
}

function assertOwnedHeadlessBrowser(home: string): void {
  const powershell = join(process.env.SystemRoot ?? "C:\\Windows", "System32", "WindowsPowerShell", "v1.0");
  const output = execFileSync(join(powershell, "powershell.exe"), ["-NoProfile", "-NonInteractive", "-Command", `
    $ErrorActionPreference = 'Stop'
    $matchesProfile = @(Get-CimInstance Win32_Process -Filter "Name = 'chrome.exe' OR Name = 'msedge.exe'" | Where-Object {
      $_.CommandLine -and $_.CommandLine.Contains($env:TJOMMIE_BROWSER_PROBE_PROFILE) -and $_.CommandLine -notmatch '--type='
    } | ForEach-Object {
      $entry = Get-Process -Id $_.ProcessId -ErrorAction Stop
      [pscustomobject]@{ pid = $_.ProcessId; headless = $_.CommandLine -match '--headless(?:=|\\s|$)'; pipe = $_.CommandLine.Contains('--remote-debugging-pipe'); tcpDebugging = $_.CommandLine.Contains('--remote-debugging-port'); visibleWindow = $entry.MainWindowHandle -ne 0 }
    })
    ConvertTo-Json -InputObject $matchesProfile -Compress
  `], {
    encoding: "utf8", windowsHide: true,
    env: { ...process.env, PSModulePath: join(powershell, "Modules"), TJOMMIE_BROWSER_PROBE_PROFILE: join(home, "chrome-profile") },
  });
  const roots: unknown = JSON.parse(output);
  expect(roots).toEqual([expect.objectContaining({ pid: expect.any(Number), headless: true, pipe: true, tcpDebugging: false, visibleWindow: false })]);
}

describe("embedded browser in the desktop side panel", () => {
  it("uses pointer and keyboard input inside the app, keeps cookies when hidden, and isolates bots", async () => {
    const { client, page, electronApp } = running();
    const create = (name: string) => client.call("bots.create", {
      name, color: "blue", shape: "circle", provider: "claude", model: "haiku", instructions: "Synthetic browser fixture", browser: true, autonomy: "ask",
    });
    const bot = await create("Embedded browser fixture");
    await openChat(page, bot.name);
    await page.evaluate(() => {
      const probe: unknown[] = [];
      (globalThis as unknown as { browserProbe: unknown[] }).browserProbe = probe;
      const original = WebSocket.prototype.send;
      const hooked = new WeakSet<WebSocket>();
      const record = (value: unknown): value is Record<string, unknown> => typeof value === "object" && value !== null;
      WebSocket.prototype.send = function(this: WebSocket, data) {
        if (!hooked.has(this)) {
          hooked.add(this);
          this.addEventListener("message", (event) => {
            try {
              const message: unknown = JSON.parse(String(event.data));
              if (!record(message) || !record(message.result)) return;
              const result = message.result;
              const state = record(result.state) ? result.state : result;
              if (typeof state.generation !== "string") return;
              probe.push({ result: true, page: state.activePageId, control: state.control, frameWidth: record(result.frame) ? result.frame.width : null });
            } catch { /* Ignore unrelated WebSocket traffic. */ }
          });
        }
        try {
          const request: unknown = JSON.parse(String(data));
          if (record(request) && typeof request.method === "string" && request.method.startsWith("browser.") && record(request.params)) {
            const p = request.params;
            if (request.method !== "browser.frame") probe.push({ method: request.method, page: p.pageId, action: p.action, width: p.width, height: p.height, control: p.control, inputKind: record(p.input) ? p.input.kind : null });
          }
        } catch { /* Do not record credentials, raw frames or input text. */ }
        return original.call(this, data);
      };
    });
    await page.getByRole("button", { name: "Open browser", exact: true }).click();
    await page.getByRole("button", { name: "Take control", exact: true }).click();
    const address = page.getByRole("textbox", { name: "Browser address", exact: true });
    await address.fill(origin);
    await page.getByRole("button", { name: "Go", exact: true }).click();
    await expect.poll(async () => (await client.call("browser.frame", { botId: bot.id })).state.tabs.some((tab) => tab.url === `${origin}/`)).toBe(true);
    const viewport = page.getByLabel("Browser viewport", { exact: true });
    await viewport.waitFor();
    await expect.poll(async () => {
      const box = await viewport.boundingBox();
      const { frame } = await client.call("browser.frame", { botId: bot.id });
      return frame && box ? frame.width / box.width : 0;
    }, { timeout: 15_000 }).toBeCloseTo(1, 1);
    const clickPage = async (x: number, y: number) => {
      const image = viewport.locator("img");
      await image.waitFor({ state: "visible" });
      // Click the picture the person actually sees. A backend frame can already reflect a resize
      // while the panel still displays the previous frame, so backend dimensions are insufficient.
      const dimensions = () => image.evaluate((element) => {
        const image = element as unknown as { naturalWidth: number; naturalHeight: number; complete: boolean };
        return { width: image.naturalWidth, height: image.naturalHeight, ready: image.complete && image.naturalWidth > 0 };
      });
      await expect.poll(async () => {
        return (await dimensions()).ready;
      }, { timeout: 15_000 }).toBe(true);
      const frame = await dimensions();
      const box = await viewport.boundingBox();
      if (!box) throw new Error("The browser viewport was not visible.");
      await viewport.click({ position: { x: x * box.width / frame.width, y: y * box.height / frame.height } });
    };
    await clickPage(80, 35);
    await viewport.pressSequentially("local-user");
    await expect.poll(() => ({ value: typedValue, events: inputEvents }), { timeout: 15_000 }).toMatchObject({ value: "local-user" });
    await clickPage(55, 90);
    await expect.poll(() => signedInAs).toBe("local-user");
    await expect.poll(() => sawCookie).toBe(true);
    await clickPage(70, 200);
    const dialog = page.getByRole("alertdialog", { name: "The page asks", exact: true });
    await dialog.getByText("Fixture confirmation", { exact: true }).waitFor({ timeout: 15_000 });
    await dialog.getByRole("button", { name: "Cancel", exact: true }).click();
    await dialog.waitFor({ state: "detached", timeout: 15_000 });
    await clickPage(70, 145);
    const popupTab = page.getByRole("tab", { name: "Browser tab Fixture popup", exact: true });
    await popupTab.waitFor();
    await page.getByRole("button", { name: "Close tab Fixture popup", exact: true }).click();
    await popupTab.waitFor({ state: "detached" });
    await page.getByRole("tab", { name: "Browser tab Fixture browser", exact: true }).waitFor();
    await viewport.locator("img").waitFor({ state: "visible" });
    await expect.poll(async () => {
      const box = await viewport.boundingBox();
      const { frame } = await client.call("browser.frame", { botId: bot.id });
      return frame && box ? frame.width / box.width : 0;
    }, { timeout: 15_000 }).toBeCloseTo(1, 1);
    await page.screenshot({ path: resolve(import.meta.dirname, "../../../docs/screenshots/phase1-embedded-browser.png") });
    await page.getByRole("button", { name: "Close browser panel", exact: true }).click();
    await expect.poll(async () => (await client.call("browser.frame", { botId: bot.id })).state.control).toBe("bot");
    await page.getByRole("button", { name: "Open browser", exact: true }).click();
    await address.waitFor();
    expect((await client.call("browser.frame", { botId: bot.id })).state.tabs.map((tab) => tab.url)).toContain(`${origin}/signed-in`);
    const other = await create("Isolated browser fixture");
    await client.call("browser.open", { botId: other.id });
    const state = await client.call("browser.control", { botId: other.id, control: "human" });
    if (!state.activePageId) throw new Error("Second browser has no page.");
    sawCookie = true;
    await client.call("browser.navigate", { botId: other.id, generation: state.generation, pageId: state.activePageId, url: `${origin}/signed-in` });
    expect(sawCookie).toBe(false);
    // Electron windows alone cannot detect a separate Chrome process. Inspect only each fixture's
    // exact profile owner and return PID/flags, never full command lines or unrelated browser data.
    assertOwnedHeadlessBrowser(bot.homeFolder);
    assertOwnedHeadlessBrowser(other.homeFolder);
    expect(electronApp.windows()).toHaveLength(1);
  });
});
