import { spawn } from "node:child_process";
import { mkdtemp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { basename, dirname, join, resolve } from "node:path";
import ts from "typescript";
import { expect, it } from "vitest";

it.skipIf(process.platform !== "win32")("finishes the first prevented Electron Quit after device cleanup, then retires its backend", async () => {
  const source = ts.createSourceFile("main.ts", await readFile(new URL("./main.ts", import.meta.url), "utf8"), ts.ScriptTarget.Latest, true);
  const findBranch = (node: ts.Node): ts.IfStatement | undefined => {
    if (ts.isCallExpression(node) && ts.isPropertyAccessExpression(node.expression) && ts.isIdentifier(node.expression.expression) && node.expression.expression.text === "app" && node.expression.name.text === "on" && ts.isStringLiteral(node.arguments[0]) && node.arguments[0].text === "will-quit") {
      const listener = node.arguments[1];
      if (ts.isArrowFunction(listener) && ts.isBlock(listener.body) && ts.isIfStatement(listener.body.statements[0])) return listener.body.statements[0];
    }
    return ts.forEachChild(node, findBranch);
  };
  const branch = findBranch(source);
  if (!branch) throw new Error("The production device-cleanup Quit branch was not found.");
  const executable: unknown = createRequire(import.meta.url)("electron");
  if (typeof executable !== "string") throw new Error("The installed Electron executable is unavailable.");
  const root = await mkdtemp(join(tmpdir(), "tjommie-quit-regression-"));
  let child: ReturnType<typeof spawn> | undefined;
  let exited: Promise<{ code: number | null; signal: NodeJS.Signals | null }> | undefined;
  try {
    for (const name of ["home", "appdata", "localappdata", "tmp", "user-data", "crash", "logs"]) await mkdir(join(root, name));
    const fixture = (await readFile(new URL("./quit-retry.fixture.cjs", import.meta.url), "utf8")).replace("/* PRODUCTION_DEVICE_QUIT_BRANCH */", branch.getText(source));
    const main = join(root, "main.cjs"); await writeFile(main, fixture);
    child = spawn(executable, [main, root], { cwd: root, windowsHide: true, stdio: ["ignore", "pipe", "pipe"], env: { PATH: "", SystemRoot: process.env.SystemRoot ?? "C:\\Windows", WINDIR: process.env.SystemRoot ?? "C:\\Windows", USERPROFILE: join(root, "home"), HOME: join(root, "home"), APPDATA: join(root, "appdata"), LOCALAPPDATA: join(root, "localappdata"), TEMP: join(root, "tmp"), TMP: join(root, "tmp") } });
    let stdout = "", stderr = "";
    child.stdout!.on("data", (bytes: Buffer) => { stdout += bytes.toString(); }); child.stderr!.on("data", (bytes: Buffer) => { stderr += bytes.toString(); });
    exited = new Promise((done, failed) => { child!.once("error", failed); child!.once("close", (code, signal) => done({ code, signal })); });
    let timer: ReturnType<typeof setTimeout> | undefined;
    const exit = await Promise.race([exited, new Promise<never>((_done, failed) => { timer = setTimeout(() => failed(new Error("Owned Electron Quit probe timed out.")), 12_000); })]).finally(() => clearTimeout(timer));
    expect(exit, stderr).toEqual({ code: 0, signal: null });
    const result = JSON.parse(stdout.trim().split(/\r?\n/).find(line => line.startsWith('{"event":"quit"')) ?? "null") as { cleanupCount: number; stopCount: number; recoveryCount: number; backendPid: number; backendExit: { code: number; signal: string | null } } | null;
    expect(result).not.toBeNull();
    expect(result).toMatchObject({ cleanupCount: 1, stopCount: 1, recoveryCount: 0, backendExit: { code: 0, signal: null } });
    expect(() => process.kill(result!.backendPid, 0)).toThrow();
  } finally {
    if (child && child.exitCode === null && child.signalCode === null) { child.kill(); await exited?.catch(() => {}); }
    if (dirname(resolve(root)) !== resolve(tmpdir()) || !basename(root).startsWith("tjommie-quit-regression-")) throw new Error("Refusing to remove an unowned test profile.");
    await rm(root, { recursive: true, force: true, maxRetries: 3 });
  }
}, 20_000);
