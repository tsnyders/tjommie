import { createHash, randomUUID } from "node:crypto";
import { DiagnosticVersion, parseDiagnosticReport } from "@tjommie/protocol";
import { AttachmentSaves } from "./attachment-save.ts";
import { speechOrigin } from "./speech-permissions.ts";

type Frame = { url: string };
type Contents = { mainFrame: Frame; getURL(): string; isDestroyed(): boolean };
type Window = { webContents: Contents; isDestroyed(): boolean };
type Event = { sender: Contents; senderFrame: Frame | null };
type Options = {
  window(): Window | null;
  production: boolean;
  appVersion(): string;
  platform: NodeJS.Platform;
  choose(): Promise<string | null>;
  watch(invalidate: () => void): () => void;
};

export function createDesktopDiagnostics(options: Options) {
  const saves = new AttachmentSaves();
  let saving = false;
  const trusted = (event: Event): Window => {
    const window = options.window();
    if (!window || window.isDestroyed() || event.sender !== window.webContents || event.sender.isDestroyed() || event.senderFrame !== window.webContents.mainFrame || !event.senderFrame || !speechOrigin(event.senderFrame.url, options.production) || !speechOrigin(event.sender.getURL(), options.production)) throw new Error("Untrusted diagnostic request.");
    return window;
  };
  return {
    info(event: Event): { appVersion: string; platform: "windows" | "macos" | "linux" } {
      trusted(event);
      const value = DiagnosticVersion.safeParse(options.appVersion());
      if (!value.success || value.data === null) throw new Error("App version is unavailable.");
      const platform = options.platform === "win32" ? "windows" : options.platform === "darwin" ? "macos" : options.platform === "linux" ? "linux" : null;
      if (!platform) throw new Error("App platform is unavailable.");
      return { appVersion: value.data, platform };
    },
    async saveReport(event: Event, input: unknown): Promise<{ status: "saved" | "cancelled" }> {
      const window = trusted(event), frameUrl = event.senderFrame!.url;
      const report = parseDiagnosticReport(input);
      if (saving) throw new Error("A diagnostic save is already open.");
      saving = true;
      const controller = new AbortController(), owner = randomUUID();
      let stopWatching: (() => void) | undefined, saveId: string | undefined;
      try {
        stopWatching = options.watch(() => controller.abort());
        const destination = await options.choose();
        if (!destination || controller.signal.aborted) return { status: "cancelled" };
        if (trusted(event) !== window || event.senderFrame!.url !== frameUrl) return { status: "cancelled" };
        const bytes = Buffer.from(report, "utf8");
        const save = await saves.start(owner, { id: randomUUID(), hostId: "diagnostics", name: "tjommie-diagnostics.json", size: bytes.length, contentType: "application/json", sha256: createHash("sha256").update(bytes).digest("hex") }, destination, controller.signal);
        saveId = save.saveId;
        await saves.append(owner, saveId, 0, bytes.toString("base64"));
        if (controller.signal.aborted || trusted(event) !== window || event.senderFrame!.url !== frameUrl) return { status: "cancelled" };
        return await saves.finish(owner, saveId);
      } catch {
        if (controller.signal.aborted) return { status: "cancelled" };
        throw new Error("The diagnostic report could not be saved.");
      } finally {
        try { if (saveId) await saves.cancel(owner, saveId); }
        catch { throw new Error("The incomplete diagnostic save could not be removed."); }
        finally { stopWatching?.(); saving = false; }
      }
    },
  };
}
