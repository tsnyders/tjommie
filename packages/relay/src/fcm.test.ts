import { generateKeyPairSync } from "node:crypto";
import { decodeJwt } from "jose";
import { afterEach, expect, it, vi } from "vitest";
import { PushData } from "@tjommie/protocol";
import { sendPush } from "./fcm.ts";

afterEach(() => vi.unstubAllGlobals());

it("signs the service-account grant, sends content-free FCM payloads, caches OAuth, and marks unregistered tokens", async () => {
  const { privateKey } = generateKeyPairSync("rsa", { modulusLength: 2048 });
  const env = {
    FCM_SERVICE_ACCOUNT: JSON.stringify({
      client_email: "relay-test@example.test", project_id: "throwaway-project",
      private_key: privateKey.export({ type: "pkcs8", format: "pem" }).toString(),
    }),
    FCM_TOKEN_ENDPOINT: "https://auth.example.test/token",
    FCM_ENDPOINT: "https://fcm.example.test/v1",
  };
  const grants: string[] = [];
  const messages: { url: string; headers: Headers; body: unknown }[] = [];
  vi.stubGlobal("fetch", vi.fn(async (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
    const url = String(input);
    if (url === env.FCM_TOKEN_ENDPOINT) {
      grants.push(new URLSearchParams(String(init?.body)).get("assertion") ?? "");
      return Response.json({ access_token: "fake-access", expires_in: 3600 });
    }
    messages.push({ url, headers: new Headers(init?.headers), body: JSON.parse(String(init?.body)) });
    return messages.length === 1 ? Response.json({ name: "sent" }) : Response.json({ error: { status: "UNREGISTERED" } }, { status: 400 });
  }));
  const data = PushData.parse({ v: "1", hostId: "99cf98ba-2638-4b16-8a6e-1691c1673b45", kind: "approval", botId: "bot-1" });
  expect(await sendPush(env, ["token-a"], data)).toEqual([{ ok: true, unregistered: false }]);
  expect(await sendPush(env, ["token-b"], data)).toEqual([{ ok: false, unregistered: true }]);
  expect(grants).toHaveLength(1);
  const claims = decodeJwt(grants[0]);
  expect(claims).toMatchObject({ iss: "relay-test@example.test", sub: "relay-test@example.test", scope: "https://www.googleapis.com/auth/firebase.messaging", aud: env.FCM_TOKEN_ENDPOINT });
  expect((claims.exp ?? 0) - (claims.iat ?? 0)).toBe(3600);
  expect(messages).toHaveLength(2);
  expect(messages[0].url).toBe("https://fcm.example.test/v1/projects/throwaway-project/messages:send");
  expect(messages[0].headers.get("authorization")).toBe("Bearer fake-access");
  expect(messages[0].body).toEqual({ message: {
    token: "token-a",
    notification: { title: "Tjommie", body: "A bot needs you" },
    data,
    android: { priority: "high", notification: { tag: "approval:bot-1", channel_id: "bots" } },
  } });
});

it("uses fixed done and failed bodies and forwards roomId without bot content", async () => {
  const { privateKey } = generateKeyPairSync("rsa", { modulusLength: 2048 });
  const env = {
    FCM_SERVICE_ACCOUNT: JSON.stringify({ client_email: "notices@example.test", project_id: "notices", private_key: privateKey.export({ type: "pkcs8", format: "pem" }).toString() }),
    FCM_TOKEN_ENDPOINT: "https://auth-notices.example.test/token",
    FCM_ENDPOINT: "https://fcm-notices.example.test/v1",
  };
  const messages: unknown[] = [];
  vi.stubGlobal("fetch", vi.fn(async (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
    if (String(input) === env.FCM_TOKEN_ENDPOINT) return Response.json({ access_token: "fake", expires_in: 3600 });
    messages.push(JSON.parse(String(init?.body)));
    return Response.json({ name: "sent" });
  }));
  for (const kind of ["done", "failed"] as const) {
    const data = PushData.parse({ v: "1", hostId: "99cf98ba-2638-4b16-8a6e-1691c1673b45", kind, botId: "bot-1", roomId: "room-1" });
    expect(await sendPush(env, ["token"], data)).toEqual([{ ok: true, unregistered: false }]);
  }
  expect(messages).toEqual(["done", "failed"].map((kind, index) => ({ message: {
    token: "token",
    notification: { title: "Tjommie", body: ["A bot finished", "A bot ran into a problem"][index] },
    data: { v: "1", hostId: "99cf98ba-2638-4b16-8a6e-1691c1673b45", kind, botId: "bot-1", roomId: "room-1" },
    android: { priority: "high", notification: { tag: `${kind}:bot-1`, channel_id: "bots" } },
  } })));
});

it("returns failed results without credentials and treats HTTP 404 as unregistered", async () => {
  const data = PushData.parse({ v: "1", hostId: "99cf98ba-2638-4b16-8a6e-1691c1673b45", kind: "limit", botId: "" });
  expect(await sendPush({}, ["token"], data)).toEqual([{ ok: false, unregistered: false }]);
  const { privateKey } = generateKeyPairSync("rsa", { modulusLength: 2048 });
  const env = { FCM_SERVICE_ACCOUNT: JSON.stringify({ client_email: "other@example.test", project_id: "other", private_key: privateKey.export({ type: "pkcs8", format: "pem" }).toString() }) };
  vi.stubGlobal("fetch", vi.fn(async (input: RequestInfo | URL): Promise<Response> => String(input).includes("oauth2.googleapis.com")
    ? Response.json({ access_token: "fake", expires_in: 3600 })
    : new Response("", { status: 404 })));
  expect(await sendPush(env, ["gone"], data)).toEqual([{ ok: false, unregistered: true }]);
});

it("cancels unused OAuth, successful FCM, and 404 response bodies", async () => {
  const { privateKey } = generateKeyPairSync("rsa", { modulusLength: 2048 });
  const secret = JSON.stringify({ client_email: "cancel@example.test", project_id: "cancel", private_key: privateKey.export({ type: "pkcs8", format: "pem" }).toString() });
  const env = { FCM_SERVICE_ACCOUNT: secret, FCM_TOKEN_ENDPOINT: "https://auth-cancel.example.test/token", FCM_ENDPOINT: "https://fcm-cancel.example.test/v1" };
  const data = PushData.parse({ v: "1", hostId: "99cf98ba-2638-4b16-8a6e-1691c1673b45", kind: "approval", botId: "bot" });
  const cancelled: number[] = [];
  let calls = 0;
  vi.stubGlobal("fetch", vi.fn(async (): Promise<Response> => {
    calls++;
    if (calls === 2) return Response.json({ access_token: "fake", expires_in: 3600 });
    const index = calls;
    return new Response(new ReadableStream({ cancel: () => { cancelled.push(index); } }), { status: calls === 1 ? 500 : calls === 4 ? 404 : 200 });
  }));
  expect(await sendPush(env, ["first"], data)).toEqual([{ ok: false, unregistered: false }]);
  expect(await sendPush(env, ["sent", "gone"], data)).toEqual([
    { ok: true, unregistered: false }, { ok: false, unregistered: true },
  ]);
  expect(cancelled).toEqual([1, 3, 4]);
});
