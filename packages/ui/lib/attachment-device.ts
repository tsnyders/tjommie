import { Capacitor, registerPlugin } from "@capacitor/core";
import { attachmentApi, type AttachmentPlugin } from "../../mobile/src/attachments";
import type { AttachmentNativeApi } from "../../protocol/src/attachments";
import { desktop } from "./desktop";
export { attachmentCompanionName as companionName } from "../../protocol/src/attachments";

let mobile: AttachmentNativeApi | null = null;
export function attachmentDevice(): AttachmentNativeApi | null {
  const native = desktop()?.attachments; if (native) return native;
  if (!Capacitor.isNativePlatform()) return null;
  mobile ??= attachmentApi(registerPlugin<AttachmentPlugin>("TjommieAttachments")); return mobile;
}
