import { describe, expect, it } from "vitest";
import { JsonRpcLines, RpcRemoteError } from "./codex-rpc.ts";

type SentMessage = {
  id?: number | string;
  method?: string;
  params?: unknown;
  result?: unknown;
  error?: { code: number; message: string };
};

function client() {
  const sent: SentMessage[] = [];
  const rpc = new JsonRpcLines((line) => sent.push(JSON.parse(line) as SentMessage));
  return { rpc, sent };
}

describe("JsonRpcLines", () => {
  it("matches responses to requests by id, in any order", async () => {
    const { rpc, sent } = client();
    const a = rpc.request("model/list", {});
    const b = rpc.request("account/read", {});
    rpc.handleLine(JSON.stringify({ id: sent[1].id, result: { account: null } }));
    rpc.handleLine(JSON.stringify({ id: sent[0].id, result: { data: [] } }));
    await expect(a).resolves.toEqual({ data: [] });
    await expect(b).resolves.toEqual({ account: null });
    expect(sent.map((m) => m.method)).toEqual(["model/list", "account/read"]);
  });

  it("rejects with the remote error (real error text from a bad thread config)", async () => {
    const { rpc, sent } = client();
    const pending = rpc.request("thread/start", {});
    rpc.handleLine(
      JSON.stringify({
        id: sent[0].id,
        error: { code: -32600, message: "failed to load configuration: invalid transport" },
      }),
    );
    await expect(pending).rejects.toBeInstanceOf(RpcRemoteError);
  });

  it("routes notifications and server requests, and ignores junk (real app-server lines)", () => {
    const { rpc } = client();
    const seen: string[] = [];
    rpc.onNotification = (method) => seen.push(`n:${method}`);
    rpc.onRequest = (id, method) => seen.push(`r:${id}:${method}`);
    rpc.handleLine(
      '{"method":"turn/started","params":{"threadId":"01a0d7d5-63cc-7c60-9053-51ebed691584","turn":{"id":"01a0d7d5-6f72-7a01-96d4-f8a64437c1b5","items":[],"itemsView":"notLoaded","status":"inProgress","error":null}}}',
    );
    rpc.handleLine(
      '{"method":"item/commandExecution/requestApproval","id":0,"params":{"kind":"command","threadId":"t","turnId":"u","itemId":"i","startedAtMs":1790327515964}}',
    );
    rpc.handleLine("WARNING: proceeding, even though we could not create PATH aliases");
    expect(seen).toEqual(["n:turn/started", "r:0:item/commandExecution/requestApproval"]);
  });

  it("writes notifications, results and errors in the app-server's shape", () => {
    const { rpc, sent } = client();
    rpc.notify("initialized");
    rpc.respond(0, { decision: "accept" });
    rpc.respondError(1, -32601, "Not supported by Tjommie");
    expect(sent).toEqual([
      { method: "initialized" },
      { id: 0, result: { decision: "accept" } },
      { id: 1, error: { code: -32601, message: "Not supported by Tjommie" } },
    ]);
  });

  it("fails every pending request when the process goes away", async () => {
    const { rpc } = client();
    const pending = rpc.request("account/read", {});
    rpc.failAll(new Error("codex app-server exited"));
    await expect(pending).rejects.toThrow("exited");
  });
});
