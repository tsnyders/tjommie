"use client";

import { useEffect, useRef, useState } from "react";
import { Dialog } from "@/components/dialog";
import { Pill } from "@/components/pill";
import { FIELD } from "@/components/controls";
import { attachmentDevice, attachmentDeviceForLocalExport } from "@/lib/attachment-device";
import { desktop } from "@/lib/desktop";
import { isAndroidApp, remoteHosts } from "@/lib/remote-hosts";
import { useApp } from "@/lib/store";
import { diagnosticReport, diagnosticVersion } from "./diagnostics";
import { saveDiagnosticReport } from "./save-report";
import { SettingsCard, SettingsProblem, SettingsRow, SettingsSection } from "./section";
import { useSystemInfo } from "./use-system-info";

export function DiagnosticsSection() {
  const host = useApp((state) => state.host);
  const connection = useApp((state) => state.connection);
  const { info, loading, refresh } = useSystemInfo();
  const [action, setAction] = useState<"copy" | "save" | null>(null);
  const [notice, setNotice] = useState("");
  const [problem, setProblem] = useState("");
  const mounted = useRef(true), activeExport = useRef<ReturnType<typeof attachmentDeviceForLocalExport>>(null);
  const native = attachmentDevice();
  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; void activeExport.current?.invalidate().catch(() => {}); };
  }, []);
  const bridge = desktop()?.diagnostics;
  const report = diagnosticReport({ device: info?.device, host: info?.host, speech: info?.speech, connection, paired: host !== null, platform: isAndroidApp() ? "android" : "browser", attachments: native !== null });
  async function exportReport(kind: "copy" | "save"): Promise<void> {
    const exportNative = kind === "save" && !bridge ? attachmentDeviceForLocalExport(() => mounted.current) : null;
    activeExport.current = exportNative;
    setAction(kind); setNotice(""); setProblem("");
    try {
      if (kind === "copy") { await navigator.clipboard.writeText(report); setNotice("Report copied."); }
      else {
        const result = bridge ? (await bridge.saveReport(report)).status : exportNative ? await saveDiagnosticReport(report, exportNative) : null;
        if (mounted.current) setNotice(result === "saved" ? "Report saved on this device." : result === "cancelled" ? "Save cancelled." : "Saving is unavailable in this app. You can copy the report.");
      }
    } catch { if (mounted.current) setProblem(kind === "copy" ? "The report could not be copied. You can select the preview and copy it." : "The report could not be saved. Try again or copy the preview."); }
    finally { activeExport.current = null; if (mounted.current) setAction(null); }
  }
  return <SettingsSection title="Diagnostics" note="Read-only checks. No model turn, upload or chat attachment is created.">
    <SettingsCard>
      <SettingsRow title="Connected host" note="The host name is shown here but excluded from the report."><span className="text-[13px] text-secondary">{host === null ? "This device's host" : remoteHosts().find((item) => item.id === host)?.name ?? "Paired PC"}</span></SettingsRow>
      <SettingsRow title="Connection"><span className="text-[13px] text-secondary">{connection.status}</span></SettingsRow>
      <SettingsRow title="Host storage"><span className="text-[13px] text-secondary">{info?.host?.storage ?? "Unknown"}</span></SettingsRow>
      <SettingsRow title="Relay" note="A connected relay does not tell us which route this device is using. Direct/relay route and historical errors are unavailable."><span className="text-[13px] text-secondary">{info?.host?.relay ?? "Unknown"}</span></SettingsRow>
      <SettingsRow title="Last connection error" note="Historical errors are not retained. The preview includes only a fixed category for the current connection issue."><span className="text-[13px] text-secondary">Unavailable</span></SettingsRow>
    </SettingsCard>
    {info?.hostUnavailable && <SettingsProblem>The host diagnostic check is unavailable. Unknown values stay in the report.</SettingsProblem>}
    <h3 className="mb-2.5 mt-9 text-[13px] font-semibold leading-[18px] text-label">Report preview</h3>
    <p className="mb-3 text-[12px] leading-[18px] text-secondary">Only versions and fixed health categories are included. Names, account identifiers, paths, URLs, tokens, pairing codes, logs, audio, transcripts, messages and file contents are excluded.</p>
    <textarea aria-label="Diagnostic report preview" readOnly value={report} spellCheck={false} className={`${FIELD} min-h-72 resize-y py-3 font-mono text-[12px]`} />
    <div className="mt-3 flex flex-wrap gap-2">
      <Pill className="max-sm:min-h-11" disabled={loading || action !== null} onClick={() => { setNotice(""); refresh(); }}>{loading ? "Checking…" : "Refresh checks"}</Pill>
      <Pill className="max-sm:min-h-11" disabled={action !== null} onClick={() => void exportReport("copy")}>{action === "copy" ? "Copying…" : "Copy report"}</Pill>
      <Pill className="max-sm:min-h-11" disabled={action !== null || !bridge && !native} onClick={() => void exportReport("save")}>{action === "save" ? "Saving…" : "Save report"}</Pill>
    </div>
    {notice && <p role="status" className="mt-3 text-[13px] text-secondary">{notice}</p>}
    {problem && <SettingsProblem>{problem}</SettingsProblem>}
  </SettingsSection>;
}

export function AboutSection() {
  const [notices, showNotices] = useState(false);
  const host = useApp((state) => state.host);
  const { info } = useSystemInfo();
  return <SettingsSection title="About" note="Tjommie connects this device to agents running on the selected host PC.">
    <SettingsCard>
      <SettingsRow title="This device's app version"><span className="text-[13px] text-secondary">{diagnosticVersion(info?.device?.appVersion) ?? "Unknown in this app"}</span></SettingsRow>
      <SettingsRow title="Connected host's app version" note={host === null ? "The host running on this device." : "The paired PC, which may have a different version."}><span className="text-[13px] text-secondary">{diagnosticVersion(info?.host?.appVersion) ?? "Unknown"}</span></SettingsRow>
      <SettingsRow title="Host daemon version"><span className="text-[13px] text-secondary">{diagnosticVersion(info?.host?.daemonVersion) ?? "Unknown"}</span></SettingsRow>
      <SettingsRow title="Updates" note="This device's automatic update state is in General. Update the host on that PC."><a className="text-[13px] text-accent underline" href="https://github.com/tsnyders/tjommie/releases/latest" target="_blank" rel="noreferrer">Latest release</a></SettingsRow>
      <SettingsRow title="Source code"><a className="text-[13px] text-accent underline" href="https://github.com/tsnyders/tjommie" target="_blank" rel="noreferrer">GitHub project</a></SettingsRow>
      <SettingsRow title="License"><a className="text-[13px] text-accent underline" href="https://github.com/tsnyders/tjommie/blob/main/LICENSE" target="_blank" rel="noreferrer">Apache License 2.0</a></SettingsRow>
    </SettingsCard>
    <div className="mt-4"><Pill className="max-sm:min-h-11" onClick={() => showNotices(true)}>Third-party notices</Pill></div>
    {notices && <ThirdPartyNotices onClose={() => showNotices(false)} />}
  </SettingsSection>;
}

function ThirdPartyNotices({ onClose }: { onClose(): void }) {
  const [text, setText] = useState<string | null>(null), [failed, setFailed] = useState(false);
  useEffect(() => {
    const abort = new AbortController();
    void fetch("/third-party-notices.txt", { signal: abort.signal }).then(async (response) => {
      if (!response.ok) throw new Error("Unavailable notices");
      const value = await response.text();
      if (value.length > 262144) throw new Error("Oversized notices");
      if (!abort.signal.aborted) setText(value);
    }).catch(() => { if (!abort.signal.aborted) setFailed(true); });
    return () => abort.abort();
  }, []);
  return <Dialog open title="Third-party notices" width={960} closable onOpenChange={(open) => { if (!open) onClose(); }}>
    {failed ? <p role="alert">Notices are unavailable in this app build.</p> : text === null ? <p role="status">Loading notices…</p> : <pre tabIndex={0} aria-label="Third-party license texts" className="mt-3 max-h-[65dvh] overflow-auto whitespace-pre-wrap break-words text-[12px] leading-[18px] text-secondary">{text}</pre>}
  </Dialog>;
}
