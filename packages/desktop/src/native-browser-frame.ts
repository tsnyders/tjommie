type Debugger = {
  sendCommand(method: string, parameters?: Record<string, unknown>): Promise<unknown>;
  on(event: "message", listener: (event: unknown, method: string, parameters: unknown) => void): unknown;
  removeListener(event: "message", listener: (event: unknown, method: string, parameters: unknown) => void): unknown;
};
const record = (value: unknown): Record<string, unknown> => value && typeof value === "object" ? value as Record<string, unknown> : {};
const IMAGE_LIMIT = 2 * 1024 * 1024;
const SCREENCAST = { format: "jpeg", quality: 60, maxWidth: 2560, maxHeight: 1600, everyNthFrame: 1 };
const BASE64 = /^[A-Za-z0-9+/]+={0,2}$/;

// A tab has one screencast, and Chromium's Page.stopScreencast is not counted: whoever stops it stops it for everyone.
// A live stream owns it while it runs. A capture (a polled browser.frame, a bot's screenshot tool) borrows it only when no
// stream does. Starting, borrowing and stopping the screencast therefore take turns per debugger session, and the stream
// count is read only inside a turn, so a capture can neither stop a stream that is starting nor begin on one that is stopping.
type ScreencastSession = { streams: number; turn: Promise<void> };
const screencasts = new WeakMap<object, ScreencastSession>();
function inTurn<T>(debuggerApi: Debugger, task: (session: ScreencastSession) => Promise<T>): Promise<T> {
  let session = screencasts.get(debuggerApi);
  if (!session) { session = { streams: 0, turn: Promise.resolve() }; screencasts.set(debuggerApi, session); }
  const shared = session, result = shared.turn.then(() => task(shared));
  shared.turn = result.then(() => {}, () => {});
  return result;
}

/** A scoped screencast makes a hidden native view paint; no screencast image is retained. While a live stream runs on this tab, the stream's screencast is used and left running. */
export function captureNativeBrowserFrame(debuggerApi: Debugger, check: () => void, parameters: Record<string, unknown> = { format: "jpeg", quality: 60, captureBeyondViewport: false }): Promise<string> {
  return inTurn(debuggerApi, (session) => capture(debuggerApi, check, parameters, session.streams > 0));
}

async function capture(debuggerApi: Debugger, check: () => void, parameters: Record<string, unknown>, streaming: boolean): Promise<string> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const expired = new Promise<never>((_resolve, reject) => { timer = setTimeout(() => reject(new Error("The native browser frame timed out.")), 5000); });
  const send = async (method: string, args?: Record<string, unknown>): Promise<Record<string, unknown>> => {
    check(); const value = record(await Promise.race([debuggerApi.sendCommand(method, args), expired])); check(); return value;
  };
  const ack = (_event: unknown, method: string, parameters: unknown): void => {
    if (method !== "Page.screencastFrame") return;
    const id = record(parameters).sessionId;
    if (typeof id === "number") void debuggerApi.sendCommand("Page.screencastFrameAck", { sessionId: id }).catch(() => {});
  };
  // While a live stream owns the screencast it already paints the view and acknowledges its frames, and must keep running.
  check(); if (!streaming) debuggerApi.on("message", ack);
  let captured: string;
  try {
    if (!streaming) await send("Page.startScreencast", SCREENCAST);
    const result = await send("Page.captureScreenshot", parameters), data = result.data;
    if (typeof data !== "string" || !BASE64.test(data) || data.length > Math.ceil(IMAGE_LIMIT / 3) * 4 || Buffer.byteLength(data, "base64") > IMAGE_LIMIT) throw new Error("The browser frame was invalid or exceeded 2 MiB.");
    captured = data;
  } finally {
    clearTimeout(timer);
    if (!streaming) {
      debuggerApi.removeListener("message", ack);
      let cleanup: ReturnType<typeof setTimeout> | undefined;
      try { await Promise.race([debuggerApi.sendCommand("Page.stopScreencast").catch(() => {}), new Promise<void>((resolve) => { cleanup = setTimeout(resolve, 500); })]); }
      finally { clearTimeout(cleanup); }
    }
  }
  check(); return captured;
}

/** The pixel size of a JPEG, read from its start-of-frame marker without decoding the image. Null if it has none. */
export function jpegSize(bytes: Uint8Array): { width: number; height: number } | null {
  if (bytes.length < 4 || bytes[0] !== 0xff || bytes[1] !== 0xd8) return null;
  let at = 2;
  while (at + 3 < bytes.length) {
    if (bytes[at] !== 0xff) return null;
    const marker = bytes[at + 1] ?? 0;
    if (marker === 0xff) { at++; continue; } // A fill byte before a marker.
    if (marker === 0x01 || (marker >= 0xd0 && marker <= 0xd9)) { at += 2; continue; } // Markers with no segment.
    const length = ((bytes[at + 2] ?? 0) << 8) | (bytes[at + 3] ?? 0);
    if (length < 2) return null;
    // SOF0 to SOF15, except the tables that share their range: DHT (C4), JPG (C8) and DAC (CC).
    if (marker >= 0xc0 && marker <= 0xcf && marker !== 0xc4 && marker !== 0xc8 && marker !== 0xcc) {
      if (at + 9 > bytes.length) return null;
      const height = ((bytes[at + 5] ?? 0) << 8) | (bytes[at + 6] ?? 0), width = ((bytes[at + 7] ?? 0) << 8) | (bytes[at + 8] ?? 0);
      return width > 0 && height > 0 ? { width, height } : null;
    }
    at += 2 + length;
  }
  return null;
}

export type NativeBrowserStreamFrame = { data: string; width: number; height: number; deviceWidth: number; deviceHeight: number };
export type NativeBrowserStreamOptions = {
  /** At most this many frames a second are forwarded (1 to 60; 60 means every frame Chromium makes). */
  maxFps: number;
  /** Throws once the stream may no longer run: the browser is closing, a private interaction began or a newer stream replaced this one. */
  check(): void;
  /** Delivers one frame. While it runs only the newest later frame waits; older ones are dropped, never queued. */
  forward(frame: NativeBrowserStreamFrame): Promise<void>;
  /** The source stopped by itself because a check or a delivery failed. Never called after `stop()`. */
  onEnd(error: Error): void;
  now?: () => number;
};
export type NativeBrowserStream = { setMaxFps(maxFps: number): void; stop(): Promise<void> };

/** A continuous screencast of one page: each frame is acknowledged to Chromium at once, then throttled to `maxFps`, validated and forwarded. */
export async function startNativeBrowserStream(debuggerApi: Debugger, options: NativeBrowserStreamOptions): Promise<NativeBrowserStream> {
  const now = options.now ?? ((): number => performance.now());
  let maxFps = options.maxFps, stopped = false, busy = false, last = Number.NEGATIVE_INFINITY;
  let waiting: NativeBrowserStreamFrame | null = null;
  // A little under the period, so timing jitter does not turn 30 fps into 15.
  const gap = (): number => maxFps >= 60 ? 0 : 1000 / maxFps - 4;
  const halt = async (): Promise<void> => {
    if (stopped) return;
    stopped = true; waiting = null; debuggerApi.removeListener("message", listener);
    // The screencast is stopped on this stream's turn, after any capture that was borrowing it has finished, and only if no other stream still owns it.
    await inTurn(debuggerApi, async (session) => {
      if (--session.streams > 0) return;
      let timer: ReturnType<typeof setTimeout> | undefined;
      try { await Promise.race([debuggerApi.sendCommand("Page.stopScreencast").catch(() => {}), new Promise<void>((resolve) => { timer = setTimeout(resolve, 500); })]); }
      finally { clearTimeout(timer); }
    });
  };
  const end = (error: Error): void => { if (!stopped) void halt().then(() => options.onEnd(error)); };
  const deliver = async (first: NativeBrowserStreamFrame): Promise<void> => {
    busy = true;
    try {
      for (let frame: NativeBrowserStreamFrame | null = first; frame && !stopped; frame = waiting, waiting = null) {
        options.check();
        await options.forward(frame);
      }
    } catch (error) { end(error instanceof Error ? error : new Error("The browser stream failed.")); }
    finally { busy = false; }
  };
  const listener = (_event: unknown, method: string, parameters: unknown): void => {
    if (method !== "Page.screencastFrame" || stopped) return;
    const body = record(parameters);
    // Chromium sends no further frame until this one is acknowledged, whether or not it is forwarded.
    if (typeof body.sessionId === "number") void debuggerApi.sendCommand("Page.screencastFrameAck", { sessionId: body.sessionId }).catch(() => {});
    const at = now();
    if (at - last < gap()) return;
    const data = body.data;
    if (typeof data !== "string" || data.length > Math.ceil(IMAGE_LIMIT / 3) * 4 || !BASE64.test(data)) return;
    // The start-of-frame marker sits in the first few hundred bytes; 65536 characters is a whole number of base64 groups.
    const size = jpegSize(Buffer.from(data.slice(0, 65536), "base64"));
    if (!size || size.width > 4096 || size.height > 4096) return;
    const metadata = record(body.metadata);
    const device = (value: unknown, fallback: number): number => typeof value === "number" && Number.isFinite(value) && value >= 1 && value <= 4096 ? Math.round(value) : fallback;
    last = at;
    const frame = { data, width: size.width, height: size.height, deviceWidth: device(metadata.deviceWidth, size.width), deviceHeight: device(metadata.deviceHeight, size.height) };
    if (busy) waiting = frame; else void deliver(frame);
  };
  options.check();
  // The start takes its turn, so a capture that began first has stopped its own screencast before this one starts, and
  // one that begins later sees the stream and leaves the screencast alone. The count rises before anyone else's turn.
  await inTurn(debuggerApi, async (session) => {
    options.check(); // The stream may have been replaced while a capture held the turn.
    debuggerApi.on("message", listener);
    session.streams++;
    try { await debuggerApi.sendCommand("Page.startScreencast", SCREENCAST); }
    catch (error) { session.streams--; debuggerApi.removeListener("message", listener); throw error; }
  });
  return { setMaxFps: (value) => { maxFps = value; }, stop: halt };
}
