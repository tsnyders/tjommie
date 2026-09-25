import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { expect, it } from "vitest";
import { cliEnv, resolveCli } from "../cli.ts";
import { startCodexServer } from "./codex-rpc.ts";

it("starts codex app-server in a fresh CODEX_HOME and reads its empty account", async () => {
  const cmd = resolveCli("codex");
  expect(cmd, "the Codex CLI must be installed").not.toBeNull();
  const home = mkdtempSync(join(tmpdir(), "tjommie codex home-"));
  const server = await startCodexServer(cmd!, cliEnv({ CODEX_HOME: home }), home);
  try {
    expect(await server.rpc.request("account/read", {})).toMatchObject({ account: null, requiresOpenaiAuth: true });
  } finally {
    server.close();
  }
});
