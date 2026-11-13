import { EventEmitter } from "node:events";
import { createServer, type Server } from "node:http";
import { afterEach, describe, expect, it, vi } from "vitest";
import { NativeBrowserStreamMessage } from "../../protocol/src/native-browser.ts";
import { captureNativeBrowserFrame, jpegSize, startNativeBrowserStream, type NativeBrowserStreamFrame, type NativeBrowserStreamOptions } from "./native-browser-frame.ts";
import { startNativeBrowserHost, type NativeBrowserHost } from "./native-browser-host.ts";

/** A minimal JPEG: SOI, a JFIF APP0 segment, a baseline SOF0 segment of the given size, then EOI. */
function jpegBytes(width: number, height: number): Buffer {
  return Buffer.from([
    0xff, 0xd8,
    0xff, 0xe0, 0x00, 0x10, 0x4a, 0x46, 0x49, 0x46, 0x00, 0x01, 0x01, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00,
    0xff, 0xc0, 0x00, 0x11, 0x08, (height >> 8) & 0xff, height & 0xff, (width >> 8) & 0xff, width & 0xff,
    0x03, 0x01, 0x22, 0x00, 0x02, 0x11, 0x01, 0x03, 0x11, 0x01,
    0xff, 0xd9,
  ]);
}
const jpegBase64 = (width: number, height: number): string => jpegBytes(width, height).toString("base64");

type Respond = (method: string) => unknown;
/** A fake Chromium debugger: an event emitter that records each command and answers it with `respond`. */
function fakeDebugger(respond: Respond = () => ({})) {
  return Object.assign(new EventEmitter(), {
    sendCommand: vi.fn(async (method: string, _parameters?: Record<string, unknown>): Promise<unknown> => respond(method)),
  });
}
type FakeDebugger = ReturnType<typeof fakeDebugger>;

/** One screencast frame as Chromium reports it. Its JPEG is `width` by 800, so tests can tell frames apart by width. */
const frame = (sessionId: number, width: number, metadata?: Record<string, unknown>): Record<string, unknown> => ({ sessionId, data: jpegBase64(width, 800), metadata });
const emit = (debuggerApi: FakeDebugger, parameters: Record<string, unknown>): void => { debuggerApi.emit("message", {}, "Page.screencastFrame", parameters); };
/** Lets every queued microtask run, so a forwarded frame has finished before the next one arrives. */
const settle = (): Promise<void> => new Promise<void>((resolve) => setImmediate(() => resolve()));
const streamOptions = (overrides: Partial<NativeBrowserStreamOptions> = {}): NativeBrowserStreamOptions => ({ maxFps: 60, check: () => {}, forward: async () => {}, onEnd: () => {}, ...overrides });
const recordingForward = () => vi.fn(async (_frame: NativeBrowserStreamFrame): Promise<void> => {});
const sentMethods = (debuggerApi: FakeDebugger): string[] => debuggerApi.sendCommand.mock.calls.map(([method]) => method);
const ackedSessions = (debuggerApi: FakeDebugger) => debuggerApi.sendCommand.mock.calls.filter(([method]) => method === "Page.screencastFrameAck").map(([, parameters]) => parameters);

describe("jpegSize", () => {
  it("reads the pixel size from the start-of-frame marker", () => {
    expect(jpegSize(jpegBytes(1280, 800))).toEqual({ width: 1280, height: 800 });
  });
  it("returns null for bytes that are not a JPEG", () => {
    expect(jpegSize(Buffer.from("not a jpeg at all"))).toBeNull();
  });
  it("returns null for an empty array", () => {
    expect(jpegSize(new Uint8Array())).toBeNull();
  });
  it("returns null for a JPEG cut off before its frame marker", () => {
    expect(jpegSize(jpegBytes(1280, 800).subarray(0, 10))).toBeNull();
  });
  it("returns null for a JPEG with no frame marker", () => {
    expect(jpegSize(Buffer.concat([jpegBytes(1280, 800).subarray(0, 20), Buffer.from([0xff, 0xd9])]))).toBeNull();
  });
  it("returns null for zero dimensions", () => {
    expect(jpegSize(jpegBytes(0, 800))).toBeNull();
    expect(jpegSize(jpegBytes(1280, 0))).toBeNull();
  });
  it("skips a DHT segment whose payload looks like a frame marker and still finds the real one", () => {
    // DHT (FF C4) with a 14-byte length; its payload holds a fake SOF0 of 16 by 32.
    const dht = Buffer.from([0xff, 0xc4, 0x00, 0x0e, 0xff, 0xc0, 0x00, 0x11, 0x08, 0x00, 0x10, 0x00, 0x20, 0x03, 0x00, 0x00]);
    expect(jpegSize(Buffer.concat([jpegBytes(1280, 800).subarray(0, 2), dht, jpegBytes(1280, 800).subarray(2)]))).toEqual({ width: 1280, height: 800 });
  });
  it("accepts fill bytes before a marker", () => {
    expect(jpegSize(Buffer.concat([jpegBytes(1280, 800).subarray(0, 20), Buffer.from([0xff]), jpegBytes(1280, 800).subarray(20)]))).toEqual({ width: 1280, height: 800 });
  });
});

describe("native browser stream source", () => {
  it("starts a jpeg screencast and adds exactly one message listener", async () => {
    const debuggerApi = fakeDebugger();
    const stream = await startNativeBrowserStream(debuggerApi, streamOptions());
    expect(debuggerApi.sendCommand).toHaveBeenCalledWith("Page.startScreencast", { format: "jpeg", quality: 60, maxWidth: 2560, maxHeight: 1600, everyNthFrame: 1 });
    expect(debuggerApi.listenerCount("message")).toBe(1);
    await stream.stop();
  });
  it("stop sends Page.stopScreencast and removes the listener", async () => {
    const debuggerApi = fakeDebugger();
    const stream = await startNativeBrowserStream(debuggerApi, streamOptions());
    await stream.stop();
    expect(debuggerApi.sendCommand).toHaveBeenLastCalledWith("Page.stopScreencast");
    expect(debuggerApi.listenerCount("message")).toBe(0);
  });
  it("a second stop sends nothing more", async () => {
    const debuggerApi = fakeDebugger();
    const stream = await startNativeBrowserStream(debuggerApi, streamOptions());
    await stream.stop();
    const sent = sentMethods(debuggerApi).length;
    await stream.stop();
    expect(sentMethods(debuggerApi)).toHaveLength(sent);
  });
  it("rejects and leaves no listener when Page.startScreencast fails", async () => {
    const debuggerApi = fakeDebugger((method) => {
      if (method === "Page.startScreencast") throw new Error("Synthetic screencast failure");
      return {};
    });
    await expect(startNativeBrowserStream(debuggerApi, streamOptions())).rejects.toThrow("Synthetic screencast failure");
    expect(debuggerApi.listenerCount("message")).toBe(0);
  });
});

describe("native browser stream acknowledgements", () => {
  it("acknowledges every frame at once, including throttled, invalid and busy ones", async () => {
    const debuggerApi = fakeDebugger();
    let open: () => void = () => {};
    const gate = new Promise<void>((resolve) => { open = resolve; });
    const forward = vi.fn(async (_frame: NativeBrowserStreamFrame): Promise<void> => { await gate; });
    let clock = 0;
    const stream = await startNativeBrowserStream(debuggerApi, streamOptions({ maxFps: 30, now: () => clock, forward }));
    emit(debuggerApi, frame(1, 1000)); // Forwarded; its delivery now waits on the gate.
    clock = 10;
    emit(debuggerApi, frame(2, 1001)); // Throttled away.
    clock = 50;
    emit(debuggerApi, frame(3, 1002)); // Past the throttle, but a delivery is busy, so it waits.
    clock = 100;
    emit(debuggerApi, { sessionId: 4, data: "not base64!" }); // Invalid.
    expect(ackedSessions(debuggerApi)).toEqual([{ sessionId: 1 }, { sessionId: 2 }, { sessionId: 3 }, { sessionId: 4 }]);
    open();
    await vi.waitFor(() => expect(forward).toHaveBeenCalledTimes(2));
    expect(forward.mock.calls.map(([received]) => received.width)).toEqual([1000, 1002]);
    await stream.stop();
  });
  it("does not acknowledge frames of other methods", async () => {
    const debuggerApi = fakeDebugger();
    const forward = recordingForward();
    const stream = await startNativeBrowserStream(debuggerApi, streamOptions({ forward }));
    debuggerApi.emit("message", {}, "Page.frameNavigated", frame(9, 1000));
    await settle();
    expect(ackedSessions(debuggerApi)).toEqual([]);
    expect(forward).not.toHaveBeenCalled();
    await stream.stop();
  });
});

describe("native browser stream validation", () => {
  it("forwards a valid frame with its jpeg size and its rounded device size from metadata", async () => {
    const debuggerApi = fakeDebugger();
    const forward = recordingForward();
    const stream = await startNativeBrowserStream(debuggerApi, streamOptions({ forward }));
    emit(debuggerApi, frame(1, 1280, { deviceWidth: 2560.4, deviceHeight: 1599.6 }));
    await settle();
    expect(forward).toHaveBeenCalledWith({ data: jpegBase64(1280, 800), width: 1280, height: 800, deviceWidth: 2560, deviceHeight: 1600 });
    await stream.stop();
  });
  it("falls back to the jpeg size when the device size is missing or not sensible", async () => {
    const debuggerApi = fakeDebugger();
    const forward = recordingForward();
    const stream = await startNativeBrowserStream(debuggerApi, streamOptions({ forward }));
    emit(debuggerApi, frame(1, 1280));
    await settle();
    emit(debuggerApi, frame(2, 1280, { deviceWidth: "wide", deviceHeight: 0 }));
    await settle();
    emit(debuggerApi, frame(3, 1280, { deviceWidth: 5000, deviceHeight: 4097 }));
    await settle();
    expect(forward.mock.calls.map(([received]) => [received.deviceWidth, received.deviceHeight])).toEqual([[1280, 800], [1280, 800], [1280, 800]]);
    await stream.stop();
  });
  it("does not forward invalid frames, but still acknowledges them", async () => {
    const debuggerApi = fakeDebugger();
    const forward = recordingForward();
    const stream = await startNativeBrowserStream(debuggerApi, streamOptions({ forward }));
    const invalid: Record<string, unknown>[] = [
      { sessionId: 1, data: "%%% not base64 %%%" },
      { sessionId: 2, data: Buffer.from("hello world").toString("base64") },
      { sessionId: 3, data: Buffer.concat([jpegBytes(1280, 800), Buffer.alloc(2 * 1024 * 1024)]).toString("base64") },
      { sessionId: 4, data: jpegBase64(4097, 800) },
      { sessionId: 5, data: jpegBase64(800, 4097) },
    ];
    for (const parameters of invalid) {
      emit(debuggerApi, parameters);
      await settle();
    }
    expect(forward).not.toHaveBeenCalled();
    expect(ackedSessions(debuggerApi)).toEqual([{ sessionId: 1 }, { sessionId: 2 }, { sessionId: 3 }, { sessionId: 4 }, { sessionId: 5 }]);
    await stream.stop();
  });
});

describe("native browser stream throttle", () => {
  it("forwards at most 30 frames a second when maxFps is 30", async () => {
    const debuggerApi = fakeDebugger();
    const forward = recordingForward();
    let clock = 0;
    const stream = await startNativeBrowserStream(debuggerApi, streamOptions({ maxFps: 30, now: () => clock, forward }));
    emit(debuggerApi, frame(1, 1000));
    await settle();
    clock = 10;
    emit(debuggerApi, frame(2, 1001));
    await settle();
    clock = 20;
    emit(debuggerApi, frame(3, 1002));
    await settle();
    clock = 33.4;
    emit(debuggerApi, frame(4, 1003));
    await settle();
    expect(forward.mock.calls.map(([received]) => received.width)).toEqual([1000, 1003]);
    await stream.stop();
  });
  it("forwards every frame at 60 fps, even 1 ms apart", async () => {
    const debuggerApi = fakeDebugger();
    const forward = recordingForward();
    let clock = 0;
    const stream = await startNativeBrowserStream(debuggerApi, streamOptions({ maxFps: 60, now: () => clock, forward }));
    emit(debuggerApi, frame(1, 1000));
    await settle();
    clock = 1;
    emit(debuggerApi, frame(2, 1001));
    await settle();
    clock = 2;
    emit(debuggerApi, frame(3, 1002));
    await settle();
    expect(forward.mock.calls.map(([received]) => received.width)).toEqual([1000, 1001, 1002]);
    await stream.stop();
  });
  it("applies setMaxFps from the next frame on", async () => {
    const debuggerApi = fakeDebugger();
    const forward = recordingForward();
    let clock = 0;
    const stream = await startNativeBrowserStream(debuggerApi, streamOptions({ maxFps: 60, now: () => clock, forward }));
    emit(debuggerApi, frame(1, 1000));
    await settle();
    clock = 1;
    emit(debuggerApi, frame(2, 1001));
    await settle();
    stream.setMaxFps(10);
    clock = 2;
    emit(debuggerApi, frame(3, 1002));
    await settle();
    clock = 50;
    emit(debuggerApi, frame(4, 1003));
    await settle();
    clock = 200;
    emit(debuggerApi, frame(5, 1004));
    await settle();
    expect(forward.mock.calls.map(([received]) => received.width)).toEqual([1000, 1001, 1004]);
    await stream.stop();
  });
});

describe("native browser stream newest frame wins", () => {
  it("while a delivery runs, only the newest later frame waits", async () => {
    const debuggerApi = fakeDebugger();
    let open: () => void = () => {};
    const gate = new Promise<void>((resolve) => { open = resolve; });
    const forward = vi.fn(async (_frame: NativeBrowserStreamFrame): Promise<void> => { await gate; });
    const stream = await startNativeBrowserStream(debuggerApi, streamOptions({ forward }));
    emit(debuggerApi, frame(1, 1000));
    await vi.waitFor(() => expect(forward).toHaveBeenCalledTimes(1));
    emit(debuggerApi, frame(2, 1001));
    emit(debuggerApi, frame(3, 1002));
    expect(ackedSessions(debuggerApi)).toEqual([{ sessionId: 1 }, { sessionId: 2 }, { sessionId: 3 }]);
    open();
    await vi.waitFor(() => expect(forward).toHaveBeenCalledTimes(2));
    await settle();
    expect(forward.mock.calls.map(([received]) => received.width)).toEqual([1000, 1002]);
    await stream.stop();
  });
});

describe("native browser stream failures", () => {
  it("a failing check ends the stream once, with its error", async () => {
    const debuggerApi = fakeDebugger();
    const failure = new Error("Synthetic browser closing");
    let closing = false;
    const forward = recordingForward();
    const onEnd = vi.fn();
    await startNativeBrowserStream(debuggerApi, streamOptions({ check: () => { if (closing) throw failure; }, forward, onEnd }));
    emit(debuggerApi, frame(1, 1000));
    await settle();
    closing = true;
    emit(debuggerApi, frame(2, 1001));
    await vi.waitFor(() => expect(onEnd).toHaveBeenCalledOnce());
    expect(onEnd).toHaveBeenCalledWith(failure);
    expect(debuggerApi.sendCommand).toHaveBeenCalledWith("Page.stopScreencast");
    expect(debuggerApi.listenerCount("message")).toBe(0);
    emit(debuggerApi, frame(3, 1002));
    await settle();
    expect(forward).toHaveBeenCalledTimes(1);
  });
  it("a rejected delivery ends the stream once, with its error", async () => {
    const debuggerApi = fakeDebugger();
    const failure = new Error("Synthetic delivery failure");
    const forward = vi.fn(async (_frame: NativeBrowserStreamFrame): Promise<void> => { throw failure; });
    const onEnd = vi.fn();
    await startNativeBrowserStream(debuggerApi, streamOptions({ forward, onEnd }));
    emit(debuggerApi, frame(1, 1000));
    await vi.waitFor(() => expect(onEnd).toHaveBeenCalledOnce());
    expect(onEnd).toHaveBeenCalledWith(failure);
    expect(debuggerApi.sendCommand).toHaveBeenCalledWith("Page.stopScreencast");
    expect(debuggerApi.listenerCount("message")).toBe(0);
    emit(debuggerApi, frame(2, 1001));
    await settle();
    expect(forward).toHaveBeenCalledTimes(1);
  });
  it("a delivery that fails after stop() does not call onEnd", async () => {
    const debuggerApi = fakeDebugger();
    let fail: (error: Error) => void = () => {};
    const pending = new Promise<void>((_resolve, reject) => { fail = reject; });
    const onEnd = vi.fn();
    const stream = await startNativeBrowserStream(debuggerApi, streamOptions({ forward: () => pending, onEnd }));
    emit(debuggerApi, frame(1, 1000));
    await stream.stop();
    fail(new Error("Synthetic late failure"));
    await settle();
    expect(onEnd).not.toHaveBeenCalled();
  });
  it("a plain stop never calls onEnd", async () => {
    const debuggerApi = fakeDebugger();
    const onEnd = vi.fn();
    const stream = await startNativeBrowserStream(debuggerApi, streamOptions({ onEnd }));
    emit(debuggerApi, frame(1, 1000));
    await settle();
    await stream.stop();
    await settle();
    expect(onEnd).not.toHaveBeenCalled();
  });
});

// A tab has one screencast and Page.stopScreencast is not counted, so a capture (a polled browser.frame or a bot's screenshot)
// and a live stream take turns on it. These cases hold each side mid-command to force every interleaving.
describe("polled capture beside a live stream", () => {
  const screenshot = { data: Buffer.alloc(100).toString("base64") };
  /** A promise settled by hand, to hold a command open. */
  const gate = (): { promise: Promise<void>; open: () => void } => {
    let open: () => void = () => {};
    const promise = new Promise<void>((resolve) => { open = resolve; });
    return { promise, open };
  };
  const answer = (method: string): unknown => method === "Page.captureScreenshot" ? screenshot : {};

  it("while a stream runs, a capture takes only its screenshot and leaves the screencast and its listener alone", async () => {
    const debuggerApi = fakeDebugger(answer);
    const stream = await startNativeBrowserStream(debuggerApi, streamOptions());
    const captured = await captureNativeBrowserFrame(debuggerApi, () => {});
    expect(captured.length).toBeGreaterThan(100);
    expect(sentMethods(debuggerApi)).toEqual(["Page.startScreencast", "Page.captureScreenshot"]);
    expect(debuggerApi.listenerCount("message")).toBe(1);
    await stream.stop();
    expect(sentMethods(debuggerApi)).toEqual(["Page.startScreencast", "Page.captureScreenshot", "Page.stopScreencast"]);
  });
  it("a capture already running when a stream starts finishes first, so nothing stops the stream after it starts", async () => {
    const shot = gate();
    const debuggerApi = fakeDebugger((method) => method === "Page.captureScreenshot" ? shot.promise.then(() => screenshot) : {});
    const poll = captureNativeBrowserFrame(debuggerApi, () => {});
    await vi.waitFor(() => expect(sentMethods(debuggerApi)).toContain("Page.captureScreenshot"));
    const starting = startNativeBrowserStream(debuggerApi, streamOptions());
    await settle();
    // The stream waits for the capture's turn; it has not touched the screencast.
    expect(sentMethods(debuggerApi)).toEqual(["Page.startScreencast", "Page.captureScreenshot"]);
    shot.open();
    await poll;
    const stream = await starting;
    // The capture's own stop comes before the stream's start, and nothing follows the start.
    expect(sentMethods(debuggerApi)).toEqual(["Page.startScreencast", "Page.captureScreenshot", "Page.stopScreencast", "Page.startScreencast"]);
    await settle();
    expect(sentMethods(debuggerApi).at(-1)).toBe("Page.startScreencast");
    await stream.stop();
    expect(sentMethods(debuggerApi).at(-1)).toBe("Page.stopScreencast");
  });
  it("a capture that arrives while a stream is starting waits for it, then borrows its screencast", async () => {
    const started = gate();
    const debuggerApi = fakeDebugger((method) => method === "Page.startScreencast" ? started.promise.then(() => ({})) : answer(method));
    const starting = startNativeBrowserStream(debuggerApi, streamOptions());
    await vi.waitFor(() => expect(sentMethods(debuggerApi)).toEqual(["Page.startScreencast"]));
    const poll = captureNativeBrowserFrame(debuggerApi, () => {});
    await settle();
    expect(sentMethods(debuggerApi)).toEqual(["Page.startScreencast"]);
    started.open();
    const stream = await starting;
    await poll;
    // No second start and no stop: the capture only took its screenshot.
    expect(sentMethods(debuggerApi)).toEqual(["Page.startScreencast", "Page.captureScreenshot"]);
    expect(debuggerApi.listenerCount("message")).toBe(1);
    await stream.stop();
  });
  it("stopping a stream waits for a capture that is borrowing its screencast", async () => {
    const shot = gate();
    const debuggerApi = fakeDebugger((method) => method === "Page.captureScreenshot" ? shot.promise.then(() => screenshot) : {});
    const stream = await startNativeBrowserStream(debuggerApi, streamOptions());
    const poll = captureNativeBrowserFrame(debuggerApi, () => {});
    await vi.waitFor(() => expect(sentMethods(debuggerApi)).toContain("Page.captureScreenshot"));
    const stopping = stream.stop();
    await settle();
    expect(sentMethods(debuggerApi)).not.toContain("Page.stopScreencast");
    shot.open();
    await poll;
    await stopping;
    expect(sentMethods(debuggerApi)).toEqual(["Page.startScreencast", "Page.captureScreenshot", "Page.stopScreencast"]);
  });
  it("a capture after the stream has stopped runs its own screencast again", async () => {
    const debuggerApi = fakeDebugger(answer);
    const stream = await startNativeBrowserStream(debuggerApi, streamOptions());
    await stream.stop();
    await captureNativeBrowserFrame(debuggerApi, () => {});
    expect(sentMethods(debuggerApi)).toEqual(["Page.startScreencast", "Page.stopScreencast", "Page.startScreencast", "Page.captureScreenshot", "Page.stopScreencast"]);
  });
  it("stops the screencast only when the last stream on the tab stops", async () => {
    const debuggerApi = fakeDebugger();
    const first = await startNativeBrowserStream(debuggerApi, streamOptions());
    const second = await startNativeBrowserStream(debuggerApi, streamOptions());
    await first.stop();
    expect(sentMethods(debuggerApi)).not.toContain("Page.stopScreencast");
    await second.stop();
    expect(sentMethods(debuggerApi).filter((method) => method === "Page.stopScreencast")).toHaveLength(1);
  });
  it("a stream that fails to start leaves the tab usable for captures", async () => {
    let starts = 0;
    const debuggerApi = fakeDebugger((method) => {
      if (method === "Page.startScreencast" && ++starts === 1) throw new Error("Synthetic screencast failure");
      return answer(method);
    });
    await expect(startNativeBrowserStream(debuggerApi, streamOptions())).rejects.toThrow("Synthetic screencast failure");
    await captureNativeBrowserFrame(debuggerApi, () => {});
    // The capture ran its own screencast, so no stream was counted as owning it.
    expect(sentMethods(debuggerApi)).toEqual(["Page.startScreencast", "Page.startScreencast", "Page.captureScreenshot", "Page.stopScreencast"]);
  });
});

type FakeHost = {
  port: number;
  posted: { authorization: string | undefined; raw: string }[];
  reply: { status: number; hang: boolean };
  close: () => Promise<void>;
};
const closeServer = async (server: Server): Promise<void> => {
  server.closeAllConnections();
  await new Promise<void>((resolve) => server.close(() => resolve()));
};
/** A stand-in for the daemon's /browser-host: GETs idle with 204 after 100 ms, POSTs are recorded and answered with `reply.status`. */
async function startFakeHost(): Promise<FakeHost> {
  const posted: FakeHost["posted"] = [];
  const reply = { status: 204, hang: false };
  const server = createServer((request, response) => {
    if (request.method === "POST") {
      let raw = "";
      request.setEncoding("utf8");
      request.on("data", (chunk: string) => { raw += chunk; });
      request.on("end", () => {
        posted.push({ authorization: request.headers.authorization, raw });
        // A hanging daemon has taken the request and never answers it.
        if (!reply.hang) response.writeHead(reply.status).end();
      });
      return;
    }
    const timer = setTimeout(() => { response.writeHead(204).end(); }, 100);
    response.on("close", () => clearTimeout(timer));
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", () => resolve()));
  const address = server.address();
  if (address === null || typeof address === "string") throw new Error("The fake host has no TCP port.");
  return { port: address.port, posted, reply, close: () => closeServer(server) };
}
const frameMessage = (sequence: number, width: number): NativeBrowserStreamMessage => ({
  kind: "stream_frame",
  botId: "bot-one",
  frame: { data: jpegBase64(width, 800), mime: "image/jpeg", width, height: 800, sequence, streamId: "stream" },
});

describe("native browser stream through the host bridge", () => {
  const token = "h".repeat(43);
  let fake: FakeHost | undefined;
  let host: NativeBrowserHost | undefined;
  afterEach(async () => {
    await host?.close();
    host = undefined;
    await fake?.close();
    fake = undefined;
  });

  it("host.send resolves true when the daemon accepts a live frame", async () => {
    fake = await startFakeHost();
    host = startNativeBrowserHost({ port: fake.port, token, execute: async () => ({}) });
    const message = frameMessage(1, 1000);
    await expect(host.send(message)).resolves.toBe(true);
    expect(fake.posted).toHaveLength(1);
    expect(fake.posted[0]?.authorization).toBe(`Bearer ${token}`);
    const body: unknown = JSON.parse(fake.posted[0]?.raw ?? "");
    expect(NativeBrowserStreamMessage.parse(body)).toEqual(message);
  }, 15_000);

  it("host.send resolves false when the daemon has nobody subscribed (404)", async () => {
    fake = await startFakeHost();
    fake.reply.status = 404;
    host = startNativeBrowserHost({ port: fake.port, token, execute: async () => ({}) });
    await expect(host.send(frameMessage(1, 1000))).resolves.toBe(false);
  }, 15_000);

  it("host.send resolves false when the daemon is unreachable", async () => {
    fake = await startFakeHost();
    host = startNativeBrowserHost({ port: fake.port, token, execute: async () => ({}) });
    await fake.close();
    await expect(host.send(frameMessage(1, 1000))).resolves.toBe(false);
  }, 15_000);

  it("host.send gives up after 2 s on a daemon that takes the request and never answers", async () => {
    fake = await startFakeHost();
    fake.reply.hang = true;
    host = startNativeBrowserHost({ port: fake.port, token, execute: async () => ({}) });
    const started = performance.now();
    await expect(host.send(frameMessage(1, 1000))).resolves.toBe(false);
    const waited = performance.now() - started;
    // It was posted, and it waited out the timeout rather than hanging (or giving up at once).
    expect(fake.posted).toHaveLength(1);
    expect(waited).toBeGreaterThanOrEqual(1900);
    expect(waited).toBeLessThan(6000);
  }, 15_000);

  it("closing the host ends a post that is waiting on a daemon that never answers", async () => {
    fake = await startFakeHost();
    fake.reply.hang = true;
    host = startNativeBrowserHost({ port: fake.port, token, execute: async () => ({}) });
    const sending = host.send(frameMessage(1, 1000));
    await vi.waitFor(() => expect(fake?.posted).toHaveLength(1));
    const started = performance.now();
    await host.close();
    await expect(sending).resolves.toBe(false);
    expect(performance.now() - started).toBeLessThan(1500);
  }, 15_000);

  it("host.send resolves false once the host is closed", async () => {
    fake = await startFakeHost();
    host = startNativeBrowserHost({ port: fake.port, token, execute: async () => ({}) });
    await host.close();
    await expect(host.send(frameMessage(1, 1000))).resolves.toBe(false);
  }, 15_000);

  it("streams three frames from a screencast source through the bridge, in order", async () => {
    fake = await startFakeHost();
    const bridge = startNativeBrowserHost({ port: fake.port, token, execute: async () => ({}) });
    host = bridge;
    const debuggerApi = fakeDebugger();
    const delivered: boolean[] = [];
    let sequence = 0;
    const stream = await startNativeBrowserStream(debuggerApi, streamOptions({
      forward: async (streamed) => {
        sequence += 1;
        delivered.push(await bridge.send({ kind: "stream_frame", botId: "bot-one", frame: { data: streamed.data, mime: "image/jpeg", width: streamed.width, height: streamed.height, sequence, streamId: "stream" } }));
      },
    }));
    const widths = [1000, 1001, 1002];
    for (const [index, width] of widths.entries()) {
      emit(debuggerApi, frame(index + 1, width));
      await vi.waitFor(() => expect(delivered).toHaveLength(index + 1), { timeout: 5_000 });
    }
    expect(delivered).toEqual([true, true, true]);
    expect(fake.posted).toHaveLength(3);
    const messages = fake.posted.map(({ raw }) => {
      const body: unknown = JSON.parse(raw);
      return NativeBrowserStreamMessage.parse(body);
    });
    expect(messages.map((message) => message.kind === "stream_frame" ? [message.frame.width, message.frame.sequence] : null)).toEqual([[1000, 1], [1001, 2], [1002, 3]]);
    await stream.stop();
  }, 15_000);
});
