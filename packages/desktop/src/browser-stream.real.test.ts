import { createServer, type Server } from "node:http";
import { expect, it } from "vitest";
import { connect, type Client } from "@tjommie/client";
import { BROWSER_START_URL, type BrowserState } from "@tjommie/protocol";
import { STREAM_FIXTURE_PATH, streamFixtureHtml } from "./browser-stream-fixture.ts";
import { meetsStreamTarget, summarizeStream } from "./browser-stream-stats.ts";
import { closeRealApp, daemonConnection, launchRealApp, openChat, type RealApp } from "./real-app.ts";

const WARM_UP_MS = 2_000;
const MEASURE_MS = 60_000;
const sleep = (ms: number): Promise<void> => new Promise((done) => setTimeout(done, ms));

// Gated to the Rust daemon: browser.stream exists only there, and the run takes about 60 s of measurement.
it.runIf(Boolean(process.env.TJOMMIE_DAEMON_EXE))("streams the 1280x800 fixture on loopback at the spec 3.9 target", async () => {
  const site: Server = createServer((req, res) => {
    const url = new URL(req.url ?? "/", "http://fixture.invalid");
    if (url.pathname !== STREAM_FIXTURE_PATH) { res.writeHead(404).end(); return; }
    res.writeHead(200, { "content-type": "text/html" }).end(streamFixtureHtml());
  });
  let app: RealApp | undefined;
  let viewer: Client | undefined;
  try {
    await new Promise<void>((resolve) => site.listen(0, "127.0.0.1", resolve));
    const address = site.address();
    if (!address || typeof address === "string") throw new Error("Fixture server has no port.");
    const origin = `http://127.0.0.1:${address.port}`;
    // Isolated accounts: the default would give the daemon the signed-in accounts folder under %LOCALAPPDATA%\Tjommie.
    app = await launchRealApp(undefined, { isolatedAccounts: true });
    const { client, page, electronApp } = app;
    // Answer the browser's start page locally so nothing leaves loopback (the native test does the same).
    await electronApp.context().route(BROWSER_START_URL, (route) => route.fulfill({ contentType: "text/html", body: "<title>Start page fixture</title><p>Search the web</p>" }));
    const bot = await client.call("bots.create", { name: "Stream fixture", color: "blue", shape: "circle", provider: "claude", model: "haiku", instructions: "Synthetic browser stream fixture", browser: true, autonomy: "ask" });
    const botId = bot.id;
    // The panel opens as in the embedded test. An unmounted view may never paint, and the stream only sends painted frames.
    await openChat(page, bot.name);
    // "Open browser" is an item of the chat header's More menu, not a button of its own.
    await page.getByRole("button", { name: "More", exact: true }).click();
    await page.getByRole("menuitem", { name: "Open browser", exact: true }).click();
    await client.call("browser.open", { botId });
    const controlled = await client.call("browser.control", { botId, control: "human" });
    // The daemon rejects navigate and resize without the current control epoch.
    const target = (state: BrowserState) => {
      if (!state.activePageId) throw new Error("The browser has no active page.");
      if (state.controlEpoch === undefined) throw new Error("The browser state has no control epoch.");
      return { botId, generation: state.generation, pageId: state.activePageId, controlEpoch: state.controlEpoch };
    };
    const navigated = await client.call("browser.navigate", { ...target(controlled), url: `${origin}${STREAM_FIXTURE_PATH}` });
    const resized = await client.call("browser.resize", { ...target(navigated), width: 1280, height: 800 });

    // The harness client keeps every event it receives; a minute of frames would stay in memory. A stream is per
    // connection anyway, so the viewer is its own connection, opened like the UI's but without recording.
    const { port, token } = await daemonConnection(page);
    const watching = await connect(port, token, { recordEvents: false });
    viewer = watching;
    const arrivals: number[] = [];
    const endings: number[] = [];
    // Each frame names its page, so a tab switch can be seen in the frames.
    const frames: { at: number; pageId: string | undefined }[] = [];
    let firstFrame: { width: number; height: number } | undefined;
    const unsubscribe = watching.subscribe((event) => {
      if (event.event !== "browser.streamFrame" || event.data.botId !== botId) return;
      const { frame } = event.data;
      if (frame === null) { endings.push(performance.now()); return; }
      arrivals.push(performance.now());
      frames.push({ at: performance.now(), pageId: frame.pageId });
      firstFrame ??= { width: frame.width, height: frame.height };
      // Every frame is acknowledged. The daemon stops sending after two unacknowledged frames.
      void watching.call("browser.streamAck", { botId, sequence: frame.sequence }).catch(() => {});
    });
    await watching.call("browser.stream", { botId, on: true, maxFps: 60 });
    const measureFrom = performance.now() + WARM_UP_MS;
    await sleep(WARM_UP_MS + MEASURE_MS);
    const measureTo = performance.now();

    // A tab switch while streaming: a page switch moves the stream to the new active page (only a private interaction, a close
    // or a failure ends it, `native-browser.ts` `#syncStream`), so frames keep coming, now from that page, and no end arrives.
    const endedBefore = endings.length;
    const framesFrom = async (pageId: string, since: number): Promise<void> => {
      for (const deadline = performance.now() + 15_000; performance.now() < deadline; await sleep(100)) {
        if (frames.some((frame) => frame.at >= since && frame.pageId === pageId)) return;
      }
      throw new Error(`No frame from page ${pageId} within 15 s of the tab switch.`);
    };
    const firstPage = resized.activePageId;
    if (!firstPage) throw new Error("The browser has no active page.");
    const openedAt = performance.now();
    const second = await client.call("browser.tab", { botId, generation: resized.generation, ...(resized.controlEpoch === undefined ? {} : { controlEpoch: resized.controlEpoch }), action: "new" });
    if (!second.activePageId || second.activePageId === firstPage) throw new Error("The new tab did not become the active page.");
    await framesFrom(second.activePageId, openedAt);
    const backAt = performance.now();
    await client.call("browser.tab", { botId, generation: second.generation, ...(second.controlEpoch === undefined ? {} : { controlEpoch: second.controlEpoch }), action: "select", pageId: firstPage });
    await framesFrom(firstPage, backAt);
    expect(endings.length, "the stream ended across a tab switch").toBe(endedBefore);

    await watching.call("browser.stream", { botId, on: false });
    unsubscribe();

    const stats = summarizeStream(arrivals.filter((time) => time >= measureFrom && time < measureTo));
    console.info(`browser-stream-result ${JSON.stringify({ ...stats, width: firstFrame?.width ?? null, height: firstFrame?.height ?? null })}`);
    expect(endings.filter((time) => time < measureTo), "the stream ended during the measurement").toHaveLength(0);
    expect(stats.frames).toBeGreaterThan(0);
    expect(meetsStreamTarget(stats), JSON.stringify(stats)).toBe(true);
  } finally {
    await viewer?.close().catch(() => {});
    try { await closeRealApp(app, false); }
    finally { if (site.listening) await new Promise<void>((done, reject) => site.close((error) => (error ? reject(error) : done()))); }
  }
}, 240_000);
