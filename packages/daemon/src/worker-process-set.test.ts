import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { expect, it } from "vitest";

it.skipIf(process.platform !== "win32")("executes the native global ownership validator and rejects unknown or unexplained worker processes", () => {
  const helper = readFileSync(new URL("./windows-worker.ps1", import.meta.url), "utf8");
  const source = /\$globalProcessSource = @'\r?\n([\s\S]*?)\r?\n'@/.exec(helper)?.[1];
  if (!source) throw new Error("Missing fixed global process validator.");
  const script = "Add-Type -TypeDefinition @'\n" + source + "\n'@\n" + "$p=[Console]::In.ReadToEnd()|ConvertFrom-Json;foreach($case in $p){try{[TjWorkerProcessSet]::Validate('S-1-5-21-1-2-3-1002',[int[]]$case.pids,[string[]]$case.sids);[Console]::Out.WriteLine('accepted')}catch{[Console]::Out.WriteLine('rejected')}}";
  const result = spawnSync(join(process.env.SystemRoot ?? "C:\\Windows", "System32", "WindowsPowerShell", "v1.0", "powershell.exe"), ["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", script], {
    input: JSON.stringify([
      { pids: [101, 102], sids: ["S-1-5-21-1-2-3-1001", "S-1-5-18"] },
      { pids: [101, 700], sids: ["S-1-5-21-1-2-3-1001", "S-1-5-21-1-2-3-1002"] },
      { pids: [101, 701], sids: ["S-1-5-21-1-2-3-1001", null] },
      { pids: [0, 4, 101], sids: [null, null, "S-1-5-21-1-2-3-1001"] },
    ]), encoding: "utf8", timeout: 10_000, windowsHide: true,
  });
  expect(result.status).toBe(0); expect(result.stderr).toBe("");
  expect(result.stdout.trim().split(/\r?\n/)).toEqual(["accepted", "rejected", "rejected", "accepted"]);
});
