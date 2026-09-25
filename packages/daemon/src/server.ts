import { timingSafeEqual } from "node:crypto";
import { createServer, type IncomingMessage } from "node:http";
import { WebSocketServer, type WebSocket } from "ws";
import { ZodError } from "zod";
import {
  ClientMessage,
  PROTOCOL_VERSION,
  methods,
  type EventData,
  type EventName,
  type MethodName,
  type ParamsOut,
  type Result,
  type ServerMessage,
} from "@tjommie/protocol";

export class RpcError extends Error {
  code: string;
  constructor(code: string, message: string) {
    super(message);
    this.code = code;
  }
}

export type Handlers = { [M in MethodName]?: (params: ParamsOut<M>) => Result<M> | Promise<Result<M>> };
export type Broadcast = <E extends EventName>(event: E, data: EventData<E>) => void;
export type Server = { port: number; broadcast: Broadcast; close: () => Promise<void> };

const HELLO_TIMEOUT_MS = 5000;

export async function startServer(opts: { port: number; token: string; handlers: Handlers; allowedOrigins: string[] }): Promise<Server> {
  const clients = new Set<WebSocket>();
  let port = opts.port;

  const http = createServer((req, res) => {
    if (req.url === "/health") {
      res.writeHead(200, { "content-type": "application/json" }).end(JSON.stringify({ ok: true, protocol: PROTOCOL_VERSION }));
      return;
    }
    res.writeHead(404).end();
  });
  const wss = new WebSocketServer({ noServer: true, maxPayload: 16 * 1024 * 1024 });

  http.on("upgrade", (req, socket, head) => {
    if (!isLocalHost(req, port) || !originAllowed(req, opts.allowedOrigins)) {
      socket.end("HTTP/1.1 403 Forbidden\r\n\r\n");
      return;
    }
    wss.handleUpgrade(req, socket, head, (ws) => accept(ws));
  });

  const send = (ws: WebSocket, message: ServerMessage) => ws.send(JSON.stringify(message));

  function accept(ws: WebSocket): void {
    let authed = false;
    const timer = setTimeout(() => ws.close(4401, "hello required"), HELLO_TIMEOUT_MS);
    ws.on("error", () => ws.terminate());
    ws.on("close", () => {
      clearTimeout(timer);
      clients.delete(ws);
    });
    ws.on("message", async (raw) => {
      let rawMessage: unknown;
      try {
        rawMessage = JSON.parse(String(raw));
      } catch {
        ws.close(4400, "bad message");
        return;
      }
      if (!authed) {
        const givenToken = typeof rawMessage === "object" && rawMessage !== null && "token" in rawMessage ? rawMessage.token : undefined;
        if (typeof givenToken !== "string" || !sameToken(givenToken, opts.token)) {
          ws.close(4401, "unauthorized");
          return;
        }
      }
      let message: ClientMessage;
      try {
        message = ClientMessage.parse(rawMessage);
      } catch {
        ws.close(4400, "bad message");
        return;
      }
      if (!authed) {
        if (message.type !== "hello" || !sameToken(message.token, opts.token)) {
          ws.close(4401, "unauthorized");
          return;
        }
        if (message.protocol !== PROTOCOL_VERSION) {
          ws.close(4003, `protocol ${PROTOCOL_VERSION} required`);
          return;
        }
        authed = true;
        clearTimeout(timer);
        clients.add(ws);
        send(ws, { type: "welcome", protocol: PROTOCOL_VERSION });
        return;
      }
      if (message.type !== "req") return;
      try {
        send(ws, { type: "res", id: message.id, ok: true, result: await call(message.method, message.params) });
      } catch (error) {
        const code = error instanceof RpcError ? error.code : error instanceof ZodError ? "bad_params" : "internal";
        send(ws, { type: "res", id: message.id, ok: false, error: { code, message: error instanceof Error ? error.message : String(error) } });
      }
    });
  }

  async function call(method: string, params: unknown): Promise<unknown> {
    // Params are validated below before the call, so the handler table can be called untyped here.
    const handler = Object.hasOwn(methods, method) ? (opts.handlers[method as MethodName] as unknown as ((p: unknown) => unknown) | undefined) : undefined;
    if (!handler) throw new RpcError("unknown_method", `Unknown method ${method}`);
    return handler(methods[method as MethodName].params.parse(params));
  }

  const broadcast: Broadcast = (event, data) => {
    const text = JSON.stringify({ type: "event", event, data });
    for (const ws of clients) ws.send(text);
  };

  await new Promise<void>((resolve, reject) => {
    http.once("error", reject);
    http.listen(opts.port, "127.0.0.1", () => resolve());
  });
  port = (http.address() as { port: number }).port;

  return {
    port,
    broadcast,
    close: () =>
      new Promise<void>((resolve) => {
        for (const ws of clients) ws.terminate();
        wss.close();
        http.close(() => resolve());
      }),
  };
}

function sameToken(given: string, expected: string): boolean {
  const a = Buffer.from(given);
  const b = Buffer.from(expected);
  return a.length === b.length && timingSafeEqual(a, b);
}

// Blocks DNS rebinding: the Host header must name this machine's loopback on our port.
function isLocalHost(req: IncomingMessage, port: number): boolean {
  return req.headers.host === `127.0.0.1:${port}` || req.headers.host === `localhost:${port}`;
}

// Browsers always send Origin, so a web page can't skip this check. Non-browser clients send none.
function originAllowed(req: IncomingMessage, allowed: string[]): boolean {
  return req.headers.origin === undefined || allowed.includes(req.headers.origin);
}
