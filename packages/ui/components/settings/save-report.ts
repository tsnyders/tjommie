import type { AttachmentNativeApi } from "../../../protocol/src/attachments";

// Android uses the same verified current-device save flow as shared files; the report never goes to the host.
export async function saveDiagnosticReport(report: string, native: AttachmentNativeApi): Promise<"saved" | "cancelled"> {
  const bytes = new TextEncoder().encode(report);
  if (bytes.length > 16 * 1024) throw new Error("The diagnostic report is too large.");
  const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", bytes));
  const save = await native.startSave({ id: crypto.randomUUID(), hostId: "diagnostics", name: "tjommie-diagnostics.json", size: bytes.length, contentType: "application/json", sha256: [...digest].map((byte) => byte.toString(16).padStart(2, "0")).join("") });
  if (!save) return "cancelled";
  try {
    const encoded = btoa(String.fromCharCode(...bytes));
    await native.appendSave(save.saveId, 0, encoded);
    return (await native.finishSave(save.saveId)).status;
  } finally { await native.cancelSave(save.saveId); }
}
