import { randomUUID, timingSafeEqual } from "node:crypto";
import { createServer, type IncomingMessage, type ServerResponse } from "node:http";
import { WebSocketServer, type WebSocket } from "ws";
import { ZodError } from "zod";
import {
  ClientMessage,
  PROTOCOL_VERSION,
  methods,
  type ErrorCode,
  type EventData,
  type EventName,
  type MethodName,
  type ParamsOut,
  type Result,
  type ServerMessage,
} from "@tjommie/protocol";
import type { RequestContext } from "./request-context.ts";

export class RpcError extends Error {
  readonly code: ErrorCode;
  constructor(code: ErrorCode, message: string) {
    super(message);
    this.code = code;
  }
}

export type Handlers = { [M in MethodName]?: (params: ParamsOut<M>, context?: RequestContext) => Result<M> | Promise<Result<M>> };
export type Broadcast = <E extends EventName>(event: E, data: EventData<E>) => void;
type ServerConfig = { token: string; handlers: Handlers; routes?: Record<string, Route> };
export type Server = { port: number; broadcast: Broadcast; activate: (config: ServerConfig) => void; stopRequests: () => void; dispatch: (method: string, params: unknown, context?: RequestContext) => Promise<unknown>; close: () => Promise<void> };
// An extra HTTP endpoint, such as the app's MCP server at /mcp.
export type Route = (req: IncomingMessage, res: ServerResponse) => void | Promise<void>;

const HELLO_TIMEOUT_MS = 5000;

export async function startServer(opts: {
  port: number;
  token: string;
  handlers: Handlers;
  allowedOrigins: string[];
  routes?: Record<string, Route>;
  deferRequests?: boolean;
}): Promise<Server> {
  const clients = new Set<WebSocket>();
  const lifetimes = new WeakMap<WebSocket, AbortController>();
  let port = opts.port;
  let config: ServerConfig = opts;
  let ready = !opts.deferRequests;
  let stopping = false;

  const http = createServer((req, res) => {
    if (!ready) {
      res.writeHead(503).end();
      return;
    }
    if (req.url === "/health") {
      res.writeHead(200, { "content-type": "application/json" }).end(JSON.stringify({ ok: true, protocol: PROTOCOL_VERSION }));
      return;
    }
    const route = config.routes?.[(req.url ?? "").split("?")[0]];
    if (route) {
      // Only programs on this machine: browsers always send an Origin header, the CLIs never do.
      if (!isLocalHost(req, port) || req.headers.origin !== undefined) {
        res.writeHead(403).end();
        return;
      }
      Promise.resolve().then(() => route(req, res)).catch((error: unknown) => {
        console.error(error);
        if (!res.headersSent) res.writeHead(500);
        res.end();
      });
      return;
    }
    res.writeHead(404).end();
  });
  const wss = new WebSocketServer({ noServer: true, maxPayload: 16 * 1024 * 1024 });

  http.on("upgrade", (req, socket, head) => {
    if (!ready) {
      socket.end("HTTP/1.1 503 Service Unavailable\r\n\r\n");
      return;
    }
    if (!isLocalHost(req, port) || !originAllowed(req, opts.allowedOrigins)) {
      socket.end("HTTP/1.1 403 Forbidden\r\n\r\n");
      return;
    }
    wss.handleUpgrade(req, socket, head, (ws) => accept(ws));
  });

  const send = (ws: WebSocket, message: ServerMessage) => ws.send(JSON.stringify(message));

  function accept(ws: WebSocket): void {
    let authed = false;
    const lifetime = new AbortController();
    lifetimes.set(ws, lifetime);
    const context: RequestContext = { connectionId: randomUUID(), principal: { kind: "local" }, signal: lifetime.signal };
    const timer = setTimeout(() => ws.close(4401, "hello required"), HELLO_TIMEOUT_MS);
    ws.on("error", () => ws.terminate());
    ws.on("close", () => {
      lifetime.abort();
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
        if (typeof givenToken !== "string" || !sameToken(givenToken, config.token)) {
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
        if (message.type !== "hello" || !sameToken(message.token, config.token)) {
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
        const result = await call(message.method, message.params, context);
        if (!lifetime.signal.aborted) send(ws, { type: "res", id: message.id, ok: true, result });
      } catch (error) {
        if (lifetime.signal.aborted) return;
        const code = error instanceof RpcError ? error.code : error instanceof ZodError ? "bad_params" : "internal";
        send(ws, { type: "res", id: message.id, ok: false, error: { code, message: error instanceof Error ? error.message : String(error) } });
      }
    });
  }

  async function call(method: string, params: unknown, context?: RequestContext): Promise<unknown> {
    if (!ready) throw new RpcError("bad_state", stopping ? "Tjommie is stopping." : "Tjommie is starting.");
    if (context?.signal.aborted) throw new RpcError("bad_state", "This connection is closed.");
    // Params are validated below before the call, so the handler table can be called untyped here.
    const handler = Object.hasOwn(methods, method) ? (config.handlers[method as MethodName] as unknown as ((p: unknown, context?: RequestContext) => unknown) | undefined) : undefined;
    if (!handler) throw new RpcError("unknown_method", `Unknown method ${method}`);
    return handler(methods[method as MethodName].params.parse(params), context);
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
    dispatch: async (method, params, context) => {
      return call(method, params, context);
    },
    stopRequests: () => { stopping = true; ready = false; },
    activate: (next) => {
      if (stopping) throw new RpcError("bad_state", "Tjommie is stopping.");
      config = next;
      ready = true;
    },
    close: () =>
      new Promise<void>((resolve) => {
        stopping = true; ready = false;
        for (const ws of clients) { lifetimes.get(ws)?.abort(); ws.terminate(); }
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
