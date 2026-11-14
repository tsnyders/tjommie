"use client";

import { useState } from "react";
import { DesktopApp } from "@/components/general/desktop-app";
import { ThisPhone } from "@/components/devices/this-phone";
import { Pill } from "@/components/pill";
import { attachmentDevice } from "@/lib/attachment-device";
import { desktop } from "@/lib/desktop";
import { isAndroidApp } from "@/lib/remote-hosts";
import { MAX_ATTACHMENT_BYTES, ATTACHMENT_CHUNK_BYTES } from "../../../protocol/src/attachments";
import { SettingsCard, SettingsProblem, SettingsRow, SettingsSection } from "./section";

export function GeneralSection() {
  const api = desktop();
  return <SettingsSection title="General" note="These app controls belong to the device you are using, even when you connect to another PC.">
    <DesktopApp />
    <SettingsCard>
      {api ? <SettingsRow title="Closing the desktop window" note="Tjommie keeps running in this device's tray. Choose Quit from the tray to stop it." /> : <SettingsRow title="Desktop startup and updates" note="These controls are available in the Windows desktop app on that device." />}
      {api && (!api.loginItem || !api.updates) && <SettingsRow title="Startup and update controls" note="Unavailable in this app version." />}
      <SettingsRow title="Connected host settings" note="Agents, Accounts, Usage and Devices refer to the selected host PC. Switching PCs changes those sections." />
    </SettingsCard>
  </SettingsSection>;
}

export function NotificationsSection() {
  return <SettingsSection title="Notifications" note="Notification choices belong to this device. Approval requests remain available inside Tjommie.">
    {isAndroidApp() ? <ThisPhone /> : <SettingsCard><SettingsRow title="Desktop notifications" note={desktop() ? "Windows controls desktop notification permission and sound. Tjommie shows notifications when its window is not focused." : "Native notification controls are unavailable in this browser. Use Tjommie on Windows or Android."} /></SettingsCard>}
  </SettingsSection>;
}

export function FilesSection() {
  const native = attachmentDevice();
  const [clearing, setClearing] = useState(false);
  const [notice, setNotice] = useState("");
  const [problem, setProblem] = useState(false);
  async function clear(): Promise<void> {
    if (!native) return;
    setClearing(true); setNotice(""); setProblem(false);
    try { await native.invalidate(); setNotice("Temporary file data cleared on this device."); }
    catch { setProblem(true); }
    finally { setClearing(false); }
  }
  return <SettingsSection title="Files" note="Previews and saves use this device. Files are read from the selected host through your existing connection.">
    <SettingsCard>
      <SettingsRow title="Download location" note="Ask where to save each file, using Save As or Android's system document picker. No folder permission is needed in advance."><span className="text-[13px] text-secondary">Ask each time</span></SettingsRow>
      <SettingsRow title="File support" note="Open shared files and save a copy on this device."><span className="text-[13px] text-secondary">{native ? "Available" : "Unavailable in this app"}</span></SettingsRow>
      <SettingsRow title="Transfer limits" note={`Files up to ${MAX_ATTACHMENT_BYTES / 1048576} MiB; ${ATTACHMENT_CHUNK_BYTES / 1024} KiB chunks. The host limits concurrent transfers.`} />
      <SettingsRow title="Preview limits" note="Markdown opens in 32 KiB sections. PDF reads use ranges up to 256 KiB. Unsupported previews still offer Download. Codec support depends on this device." />
      <SettingsRow title="Temporary file data" note="Closes previews and cancels unfinished downloads on this device. Saved downloads, delivered host files, chats, accounts and worktrees are kept."><Pill className="max-sm:min-h-11" disabled={!native || clearing} onClick={() => void clear()}>{clearing ? "Clearing…" : "Clear temporary data"}</Pill></SettingsRow>
    </SettingsCard>
    {notice && <p role="status" className="mt-3 text-[13px] text-secondary">{notice}</p>}
    {problem && <SettingsProblem>Temporary file data could not be cleared. Close open previews and try again.</SettingsProblem>}
  </SettingsSection>;
}
