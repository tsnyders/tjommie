import { Capacitor, registerPlugin } from "@capacitor/core";
import { attachmentApi, type AttachmentPlugin } from "../../mobile/src/attachments";
import type { AttachmentNativeApi } from "../../protocol/src/attachments";
import { desktop } from "./desktop";
import { captureAttachmentSaveHeartbeat } from "./connection";
export { attachmentCompanionName as companionName } from "../../protocol/src/attachments";

let mobile: AttachmentNativeApi | null = null;
let plugin: AttachmentPlugin | null = null;
export function attachmentDevice(): AttachmentNativeApi | null {
  const native = desktop()?.attachments; if (native) return native;
  if (!Capacitor.isNativePlatform()) return null;
  mobile ??= attachmentApi(plugin ??= registerPlugin<AttachmentPlugin>("TjommieAttachments"), captureAttachmentSaveHeartbeat); return mobile;
}

// Only trusted device-generated reports use this factory; remote attachment metadata cannot select it.
export function attachmentDeviceForLocalExport(isCurrent: () => boolean): AttachmentNativeApi | null {
  const native = desktop()?.attachments; if (native) return native;
  if (!Capacitor.isNativePlatform()) return null;
  const page = typeof document === "undefined" ? null : document;
  const current = () => isCurrent() && (page === null || page === document);
  return attachmentApi(plugin ??= registerPlugin<AttachmentPlugin>("TjommieAttachments"), () => {
    if (!current()) return null;
    return async (signal) => !signal.aborted && current();
  });
}
