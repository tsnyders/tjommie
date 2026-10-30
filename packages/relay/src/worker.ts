import { DurableObject } from "cloudflare:workers";
import { z } from "zod";
import { HostRoomCore, type Attachment, type Sock } from "./room.ts";
import { sendPush, type FcmEnv } from "./fcm.ts";

interface Env extends FcmEnv { HOSTS: DurableObjectNamespace<HostRoom> }
interface WorkerSock extends Sock { socket: WebSocket }

export class HostRoom extends DurableObject<Env> {
  private readonly sockets = new Map<WebSocket, WorkerSock>();
  private readonly upgrading = new Map<WebSocket, number | null>();
  private readonly rejections = new Map<WebSocket, { code: number; at: number }>();
  private readonly core: HostRoomCore;

  constructor(ctx: DurableObjectState, env: Env) {
    super(ctx, env);
    const hostId = ctx.id.name;
    if (!hostId) throw new Error("Host name is required.");
    for (const socket of ctx.getWebSockets()) {
      const rejection = this.rejection(socket);
      if (rejection) this.rejections.set(socket, rejection);
    }
    this.core = new HostRoomCore({
      hostId,
      now: () => Date.now(),
      random: () => crypto.getRandomValues(new Uint8Array(32)),
      registerKey: (key) => {
        ctx.storage.sql.exec("CREATE TABLE IF NOT EXISTS login_key (id INTEGER PRIMARY KEY CHECK (id = 1), public_key TEXT NOT NULL)");
        ctx.storage.sql.exec("INSERT OR IGNORE INTO login_key (id, public_key) VALUES (1, ?)", key);
        return ctx.storage.sql.exec<{ public_key: string }>("SELECT public_key FROM login_key WHERE id = 1").one().public_key;
      },
      pushEnabled: Boolean(env.FCM_SERVICE_ACCOUNT),
      push: (tokens, data) => sendPush(env, tokens, data),
      sockets: ctx.getWebSockets().filter((socket) => !this.rejection(socket)).map((socket) => this.wrap(socket)),
    });
    ctx.setWebSocketAutoResponse(new WebSocketRequestResponsePair("ping", "pong"));
  }

  private wrap(socket: WebSocket): WorkerSock {
    const existing = this.sockets.get(socket);
    if (existing) return existing;
    const wrapped: WorkerSock = {
      socket,
      send: (message) => socket.send(message),
      close: (code) => {
        if (this.upgrading.has(socket)) this.upgrading.set(socket, code);
        else socket.close(code);
      },
      getAttachment: () => socket.deserializeAttachment() as Attachment | null,
      setAttachment: (value) => socket.serializeAttachment(value),
    };
    this.sockets.set(socket, wrapped);
    return wrapped;
  }

  private lastPing(): number | null {
    const control = this.core.controlSocket() as WorkerSock | null;
    return control ? this.ctx.getWebSocketAutoResponseTimestamp(control.socket)?.getTime() ?? null : null;
  }

  private rejection(socket: WebSocket): { code: number; at: number } | null {
    const value: unknown = socket.deserializeAttachment();
    if (typeof value !== "object" || value === null) return null;
    const fields = value as Record<string, unknown>;
    return fields.role === "reject" && typeof fields.code === "number" && typeof fields.at === "number"
      ? { code: fields.code, at: fields.at } : null;
  }

  private async setNextAlarm(extraDeadline?: number): Promise<void> {
    let deadline = this.core.nextDeadline(this.lastPing());
    if (extraDeadline !== undefined) deadline = deadline === null ? extraDeadline : Math.min(deadline, extraDeadline);
    for (const rejection of this.rejections.values()) deadline = deadline === null ? rejection.at : Math.min(deadline, rejection.at);
    if (deadline === null) await this.ctx.storage.deleteAlarm();
    else await this.ctx.storage.setAlarm(deadline);
  }

  async fetch(request: Request): Promise<Response> {
    if (request.headers.get("upgrade")?.toLowerCase() !== "websocket") return new Response("WebSocket required", { status: 426 });
    const url = new URL(request.url);
    const path = url.pathname.split("/");
    const pair = new WebSocketPair();
    const [client, server] = Object.values(pair);
    this.ctx.acceptWebSocket(server);
    this.upgrading.set(server, null);
    const sock = this.wrap(server);
    const ip = request.headers.get("CF-Connecting-IP") ?? "unknown";
    if (path[4] === "control") this.core.connectControl(sock, url.host, ip);
    else if (path[4] === "connect") this.core.connectDevice(sock, ip);
    else if (path[4] === "channels" && path[5]) this.core.connectChannel(sock, path[5], ip);
    else sock.close(1008);
    const rejectedCode = this.upgrading.get(server);
    this.upgrading.delete(server);
    // A close during upgrade can lose its code locally, so retry after the 101 response.
    const rejectionAt = rejectedCode !== null && rejectedCode !== undefined ? Date.now() + 500 : undefined;
    if (rejectionAt !== undefined && rejectedCode !== null && rejectedCode !== undefined) {
      const rejection = { code: rejectedCode, at: rejectionAt };
      this.rejections.set(server, rejection);
      server.serializeAttachment({ role: "reject", ...rejection });
    }
    await this.setNextAlarm(rejectionAt);
    return new Response(null, { status: 101, webSocket: client });
  }

  async webSocketMessage(socket: WebSocket, message: string | ArrayBuffer): Promise<void> {
    const rejection = this.rejections.get(socket) ?? this.rejection(socket);
    if (rejection) { this.rejections.delete(socket); socket.serializeAttachment(null); socket.close(rejection.code); return; }
    if (await this.core.onMessage(this.wrap(socket), message)) await this.setNextAlarm();
  }

  async webSocketClose(socket: WebSocket): Promise<void> {
    this.rejections.delete(socket);
    this.core.onClose(this.wrap(socket));
    this.sockets.delete(socket);
    await this.setNextAlarm();
  }

  async webSocketError(socket: WebSocket): Promise<void> {
    this.rejections.delete(socket);
    this.core.onClose(this.wrap(socket));
    this.sockets.delete(socket);
    await this.setNextAlarm();
  }

  async alarm(): Promise<void> {
    for (const [socket, rejection] of this.rejections) {
      if (rejection.at <= Date.now()) {
        this.rejections.delete(socket);
        socket.serializeAttachment(null);
        socket.close(rejection.code);
      }
    }
    this.core.sweep(this.lastPing());
    await this.setNextAlarm();
  }
}

export default {
  fetch(request: Request, env: Env): Promise<Response> | Response {
    const url = new URL(request.url);
    if (request.method === "GET" && url.pathname === "/v1/health")
      return Response.json({ ok: true, relay: "tjommie-relay/1" });
    const route = /^\/v1\/hosts\/([^/]+)\/(control|connect|channels\/[A-Za-z0-9_-]{43})$/.exec(url.pathname);
    if (!route) return new Response("Not found", { status: 404 });
    if (!z.uuid().safeParse(route[1]).success) return new Response("Invalid host id", { status: 400 });
    if (request.method !== "GET") return new Response("Method not allowed", { status: 405 });
    if (request.headers.get("upgrade")?.toLowerCase() !== "websocket") return new Response("WebSocket required", { status: 426 });
    return env.HOSTS.get(env.HOSTS.idFromName(route[1])).fetch(request);
  },
};
