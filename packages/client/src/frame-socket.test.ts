import { afterEach, expect, it, vi } from "vitest";
import { relayConnectUrl, relayWebSocketUrl, webSocketFrames } from "./frame-socket.ts";

class TestWebSocket extends EventTarget {
  static instances: TestWebSocket[] = [];
  static readonly OPEN = 1;
  binaryType = "blob";
  readyState = 0;
  bufferedAmount = 0;
  sent: Uint8Array[] = [];
  readonly url: string;
  constructor(url: string) { super(); this.url = url; TestWebSocket.instances.push(this); }
  open(): void { this.readyState = 1; this.dispatchEvent(new Event("open")); }
  send(frame: Uint8Array): void { this.sent.push(frame); }
  close(): void { this.remoteClose(1000); }
  remoteClose(code: number): void {
    this.readyState = 3;
    const event = new Event("close");
    Object.defineProperty(event, "code", { value: code });
    this.dispatchEvent(event);
  }
  error(): void { this.dispatchEvent(new Event("error")); }
  receive(data: unknown): void { this.dispatchEvent(new MessageEvent("message", { data })); }
}

afterEach(() => { vi.useRealTimers(); vi.unstubAllGlobals(); TestWebSocket.instances.length = 0; });

it("builds the device WebSocket URL from an HTTPS relay base", () => {
  expect(relayConnectUrl("https://r.example/", "99cf98ba-2638-4b16-8a6e-1691c1673b45"))
    .toBe("wss://r.example/v1/hosts/99cf98ba-2638-4b16-8a6e-1691c1673b45/connect");
});

it("replaces any base path, query and hash for every relay leg", () => {
  expect(relayWebSocketUrl("https://r.example/old/path?key=value#section", "/v1/hosts/id/control"))
    .toBe("wss://r.example/v1/hosts/id/control");
  expect(relayConnectUrl("https://r.example/old/path?key=value#section", "99cf98ba-2638-4b16-8a6e-1691c1673b45"))
    .toBe("wss://r.example/v1/hosts/99cf98ba-2638-4b16-8a6e-1691c1673b45/connect");
});

it("uses one binary WebSocket message per frame and closes cleanly", async () => {
  vi.stubGlobal("WebSocket", TestWebSocket);
  const opening = webSocketFrames("wss://r.example/connect", 1000);
  const socket = TestWebSocket.instances[0];
  expect(socket.binaryType).toBe("arraybuffer");
  socket.open();
  const wire = await opening;
  const received: Uint8Array[] = [];
  const closed = vi.fn();
  wire.onFrame((frame) => received.push(frame));
  wire.onClose(closed);
  wire.send(new Uint8Array([1, 2]));
  expect(socket.sent).toEqual([new Uint8Array([1, 2])]);
  socket.receive(new Uint8Array([3, 4]).buffer);
  expect(received).toEqual([new Uint8Array([3, 4])]);
  wire.close();
  expect(closed).toHaveBeenCalledOnce();
});

it("closes an unopened socket when its timeout expires", async () => {
  vi.useFakeTimers();
  vi.stubGlobal("WebSocket", TestWebSocket);
  const opening = webSocketFrames("wss://r.example/connect", 100);
  const rejected = expect(opening).rejects.toThrow(/timed out/i);
  await vi.advanceTimersByTimeAsync(100);
  await rejected;
  expect(TestWebSocket.instances[0].readyState).toBe(3);
});

it("passes the relay close code to current and late listeners", async () => {
  vi.stubGlobal("WebSocket", TestWebSocket);
  const opening = webSocketFrames("wss://r.example/connect", 1000);
  const socket = TestWebSocket.instances[0]; socket.open();
  const wire = await opening, codes: (number | undefined)[] = [];
  wire.onClose((code) => codes.push(code));
  socket.remoteClose(4404);
  wire.onClose((code) => codes.push(code));
  await Promise.resolve();
  expect(codes).toEqual([4404, 4404]);
});

it("closes the leg on text, empty or oversized inbound frames", async () => {
  vi.stubGlobal("WebSocket", TestWebSocket);
  for (const data of ["text", new ArrayBuffer(0), new ArrayBuffer(65_536)]) {
    const opening = webSocketFrames("wss://r.example/connect", 1000);
    const socket = TestWebSocket.instances.at(-1)!; socket.open();
    const wire = await opening, closed = vi.fn(); wire.onClose(closed);
    socket.receive(data);
    expect(socket.readyState).toBe(3);
    expect(closed).toHaveBeenCalledOnce();
  }
});

it("rejects oversized sends and a send above the 32 MiB backlog cap", async () => {
  vi.stubGlobal("WebSocket", TestWebSocket);
  for (const [buffered, frame] of [[0, new Uint8Array(65_536)], [32 * 1024 * 1024, new Uint8Array([1])]] as const) {
    const opening = webSocketFrames("wss://r.example/connect", 1000);
    const socket = TestWebSocket.instances.at(-1)!; socket.open(); socket.bufferedAmount = buffered;
    const wire = await opening;
    expect(() => wire.send(frame)).toThrow();
    expect(socket.readyState).toBe(3);
    expect(socket.sent).toEqual([]);
  }
});

it("rejects a WebSocket error before open", async () => {
  vi.stubGlobal("WebSocket", TestWebSocket);
  const opening = webSocketFrames("wss://r.example/connect", 1000);
  const rejected = expect(opening).rejects.toThrow(/failed/i);
  TestWebSocket.instances[0].error();
  await rejected;
  expect(TestWebSocket.instances[0].readyState).toBe(3);
});
