import assert from "node:assert/strict";
import { randomUUID, X509Certificate } from "node:crypto";
import { readFileSync } from "node:fs";
import { createServer } from "node:https";
import { join } from "node:path";
import { app, BrowserWindow, type WebContents } from "electron";
import { BrowserState } from "../../../packages/protocol/src/browser.ts";
import { LoginEntry, type VaultHostAction, type VaultFillTarget } from "../../../packages/protocol/src/vault.ts";
import { NativeLoginVault } from "../../../packages/desktop/src/login-vault.ts";
import { NativeBrowserEngine } from "../../../packages/desktop/src/native-browser.ts";
import { nativeBrowserAutomation } from "../../../packages/desktop/src/native-browser-automation.ts";
import { VaultRecords, type VaultSecret } from "../../../packages/desktop/src/vault-storage.ts";

const configuredRoot = process.env.TJOMMIE_PHASE4_VAULT_ROOT;
if (!configuredRoot) throw new Error("The vault probe needs a disposable root.");
const root: string = configuredRoot;
app.setPath("userData", join(root, "electron"));
const certificate = readFileSync(join(root, "cert.pem")), key = readFileSync(join(root, "key.pem")), fingerprint = new X509Certificate(certificate).fingerprint256.replaceAll(":", "").toLowerCase();
const html = '<!doctype html><title>Synthetic login</title><form method="post" action="/session"><label>Username<input name="username" autocomplete="username"></label><label>Password<input name="password" type="password" autocomplete="current-password"></label><button>Sign in</button></form><script>document.querySelector("input[type=password]").addEventListener("input",e=>{document.title=e.target.value;console.log(e.target.value)})</script>';
const sites = [0, 1].map(() => createServer({ cert: certificate, key }, (_request, response) => response.writeHead(200, { "content-type": "text/html" }).end(html)));
const origins = new Set<string>();
app.on("certificate-error", (event, contents, url, _error, cert, callback) => {
  let expected = false; try { expected = new X509Certificate(cert.data).fingerprint256.replaceAll(":", "").toLowerCase() === fingerprint; } catch { /* Reject an unrecognized certificate. */ }
  if (contents.session.storagePath?.startsWith(root) && origins.has(new URL(url).origin) && expected) { event.preventDefault(); callback(true); } else callback(false);
});
let owner: BrowserWindow | undefined, engine: NativeBrowserEngine | undefined, records: VaultRecords | undefined, vault: NativeLoginVault | undefined;
async function run(): Promise<void> {
  try {
    await app.whenReady();
    for (const site of sites) { await new Promise<void>((done) => site.listen(0, "127.0.0.1", done)); const address = site.address(); assert(address && typeof address !== "string"); origins.add(`https://127.0.0.1:${address.port}`); }
    const [origin, otherOrigin] = [...origins]; assert(origin && otherOrigin);
    owner = new BrowserWindow({ show: false, width: 900, height: 700, webPreferences: { sandbox: true, contextIsolation: true, nodeIntegration: false } });
    const states: unknown[] = [], notices: unknown[] = [], results: unknown[] = [];
    const instance = new NativeBrowserEngine({ window: owner, profileRoot: join(root, "profiles"), automation: nativeBrowserAutomation, onState: (state) => states.push(state), onNotice: (notice) => notices.push(notice) }); engine = instance;
    const bot = { id: "vault-synthetic", name: "Synthetic", homeFolder: join(root, "bot") };
    await instance.open(bot); await instance.resize(bot, 640, 480);
    const contents = await instance.withAutomation(bot, {}, async (page): Promise<WebContents> => { await page.loadURL(origin); return page; });
    await nativeBrowserAutomation(instance, bot, { kind: "tool", name: "browser_console_messages", args: {} });
    const secrets = new Map<string, VaultSecret>(), synthetic = `Synthetic-vault-${randomUUID()}`;
    let reads = 0, navigateOnConfirmation = false;
    const databasePath = join(root, "logins.sqlite"), database = new VaultRecords(databasePath); records = database;
    const options = {
      records: database,
      credentials: { write: async (reference: string, secret: VaultSecret) => { secrets.set(reference, { ...secret }); }, read: async (reference: string) => { reads++; const value = secrets.get(reference); return value ? { ...value } : null; }, delete: async (reference: string) => { secrets.delete(reference); } },
      // This fixture tests native fill, not Windows isolation or an actual person's confirmation.
      isolation: { unavailableReason: async () => null, acquire: async () => ({ assert: () => {}, release: () => {} }) },
      entry: { entry: async () => ({ name: "Synthetic account", origin, secret: { username: "synthetic-user", password: synthetic } }), confirm: async (request: { kind: string }) => { if (navigateOnConfirmation && request.kind === "fill") await contents.loadURL(otherOrigin); return true; } },
      browser: { destination: (target: VaultFillTarget, controller: string) => instance.vaultDestination(bot, target, controller), begin: (target: VaultFillTarget, controller: string, exact: string, check: () => void) => instance.beginVault(bot, target, controller, exact, check), fill: (_id: string, epoch: number, secret: VaultSecret, check: () => void) => instance.writeVault(bot, epoch, secret, check), finish: (_id: string, generation: string, epoch: number, mode: "clear" | "close", check: () => void) => instance.finishVault(bot, generation, epoch, mode, check) },
    };
    const service = new NativeLoginVault(options); vault = service;
    const call = async (action: VaultHostAction, controllerId = "local", current = service): Promise<unknown> => { try { const result = await current.command({ controllerId, action }); results.push(result); return result; } catch (error) { results.push(error instanceof Error ? error.message : "failure"); throw error; } };
    await call({ kind: "unlock" }); const saved = await call({ kind: "entry", botId: bot.id }) as { entry: unknown }, entry = LoginEntry.parse(saved.entry);
    const target = (state: unknown): VaultFillTarget => { const value = BrowserState.parse(state); assert(value.activePageId && value.controlEpoch !== undefined); return { botId: bot.id, generation: value.generation, pageId: value.activePageId, controlEpoch: value.controlEpoch }; };
    const controller = "rpc:synthetic-vault", current = target(await instance.control(bot, "human", controller)), started = performance.now();
    const filled = await call({ kind: "fill", ...current, id: entry.id, revision: entry.revision }, controller) as { status: string; privateEpoch: number };
    assert.equal(filled.status, "filled"); assert.equal(reads, 1);
    const fillMs = Math.round(performance.now() - started);
    assert.equal(await contents.executeJavaScript("document.querySelector('input[name=username]').value==='synthetic-user'&&document.querySelector('input[type=password]').value===" + JSON.stringify(synthetic)), true);
    const privateFrame = await instance.frame(bot), privateTool = await nativeBrowserAutomation(instance, bot, { kind: "tool", name: "browser_evaluate", args: { function: "() => document.querySelector('input[type=password]').value" } });
    assert.equal(privateFrame.frame, null); assert(privateTool.isError);
    await call({ kind: "disconnect" }, controller); await call({ kind: "lock" });
    assert(instance.state(bot.id).privateActive && instance.state(bot.id).control === "human");
    await call({ kind: "finish", botId: bot.id, generation: current.generation, privateEpoch: filled.privateEpoch, mode: "clear" });
    assert.equal(await contents.executeJavaScript("[...document.querySelectorAll('input')].every(n=>n.value==='')"), true);
    assert(!instance.state(bot.id).privateActive && instance.state(bot.id).control === "human");
    await instance.control(bot, "bot");
    const resumedConsole = await nativeBrowserAutomation(instance, bot, { kind: "tool", name: "browser_console_messages", args: { all: true } }); results.push(resumedConsole);
    await call({ kind: "unlock" }); navigateOnConfirmation = true;
    const late = target(await instance.control(bot, "human"));
    await assert.rejects(call({ kind: "fill", ...late, id: entry.id, revision: entry.revision }), /could not finish/); assert.equal(reads, 1, "Navigation after confirmation must refuse before another secret read.");
    const restarted = new NativeLoginVault(options); assert.equal((await restarted.status()).state, "locked");
    await call({ kind: "delete", id: entry.id, revision: entry.revision }, "local", restarted); assert.equal(secrets.size, 0); assert.equal(database.list().length, 0); restarted.close();
    for (const file of [databasePath, databasePath + "-wal"]) assert(!readFileSync(file).includes(Buffer.from(synthetic)), "Credential plaintext must be absent from metadata and WAL.");
    for (const [name, values] of Object.entries({ states, notices, results, privateFrame, privateTool })) assert(!JSON.stringify(values).includes(synthetic), `The checked ${name} output sink must not contain the synthetic secret.`);
    assert(!owner.isVisible() && owner.contentView.children.every((view) => !view.getVisible()));
    process.stdout.write("PHASE4_VAULT_RESULT " + JSON.stringify({ electron: process.versions.electron, fillMs, hiddenOwner: true, hiddenChildren: true, exactHttpsFill: true, navigationAfterConfirmationRefused: true, checkedSinks: ["state", "notice", "RPC-results-errors", "private-tool", "private-frame", "resumed-console", "metadata-db-wal"], privateUntilExplicitClear: true, clearLeavesHumanControl: true, restartLocked: true, deleteRemovedSyntheticCredential: true, credentialStore: "synthetic in-memory only", nativeEntryWindowTested: false, windowsIsolationTested: false, remoteTransportTested: false }) + "\n");
  } catch (error) { process.exitCode = 1; process.stderr.write((error instanceof Error ? error.stack : String(error)) + "\n"); }
  finally { vault?.close(); records?.close(); await engine?.closeAll(); owner?.destroy(); for (const site of sites) await new Promise<void>((done) => site.close(() => done())); app.exit(Number(process.exitCode ?? 0)); }
}
void run();
