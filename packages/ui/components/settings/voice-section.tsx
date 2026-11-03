"use client";

import { useEffect, useRef, useState } from "react";
import { SpeechEvent, SpeechStatus } from "../../../protocol/src/speech";
import { Pill } from "@/components/pill";
import { localSpeech } from "@/lib/local-speech";
import { SettingsCard, SettingsProblem, SettingsRow, SettingsSection } from "./section";

function preference(key: string): string { try { return localStorage.getItem(`tjommie.voice.${key}`) ?? ""; } catch { return ""; } }
const SELECT = "min-h-11 max-w-full rounded-[10px] border border-input-stroke bg-canvas px-3 text-[13px] text-text";

export function VoiceSection() {
  const api = localSpeech();
  const [status, setStatus] = useState<SpeechStatus | null>(null);
  const [language, setLanguage] = useState(() => preference("language"));
  const [device, setDevice] = useState(() => preference("inputDevice"));
  const [devices, setDevices] = useState<MediaDeviceInfo[]>([]);
  const [working, setWorking] = useState<string | null>(null);
  const [problem, setProblem] = useState("");
  const [notice, setNotice] = useState("");
  const alive = useRef(true), operation = useRef(0);
  const modelOwner = useRef<{ requestId?: string }>({});
  const running = useRef<string | null>(null);
  useEffect(() => {
    let cancelled = false;
    alive.current = true;
    running.current = null;
    const owner: { requestId?: string } = {}; modelOwner.current = owner;
    operation.current++;
    const show = (value: unknown): void => {
      const checked = SpeechStatus.safeParse(value);
      if (!cancelled && checked.success) setStatus(checked.data);
    };
    void api?.status().then(show, () => { if (!cancelled) setProblem("Local speech status is unavailable in this app."); });
    const remove = api?.onEvent((value) => {
      const checked = SpeechEvent.safeParse(value);
      if (checked.success && checked.data.kind === "status") show(checked.data.status);
    });
    return () => { cancelled = true; alive.current = false; if (owner.requestId) void api?.releaseModelRequest?.({ requestId: owner.requestId }).catch(() => {}); remove?.(); };
  }, [api]);

  function savePreference(key: "language" | "inputDevice", value: string): void {
    try {
      localStorage.setItem(`tjommie.voice.${key}`, value);
      if (key === "language") setLanguage(value); else setDevice(value);
      setProblem("");
    } catch { setProblem("This device could not save the voice choice."); }
  }

  async function microphones(): Promise<void> {
    try {
      const permission = await navigator.permissions.query({ name: "microphone" as PermissionName });
      if (permission.state !== "granted") { setNotice("Start dictation in a conversation to grant microphone access, then refresh this list."); return; }
      const found = (await navigator.mediaDevices.enumerateDevices()).filter((entry) => entry.kind === "audioinput" && entry.deviceId && entry.label);
      if (alive.current) { setDevices(found); setNotice(found.length ? "Microphones refreshed." : "No named microphone is available. The system default is still selected when recording starts."); }
    } catch { if (alive.current) setNotice("Microphone names are unavailable. Start dictation with the system default in a conversation."); }
  }

  async function run(action: "downloadModel" | "cancelModelDownload" | "removeModel" | "refresh"): Promise<void> {
    if (!api || running.current !== null && action !== "cancelModelDownload") return;
    const current = ++operation.current;
    running.current = action;
    setWorking(action); setProblem(""); setNotice("");
    try {
      if (action === "downloadModel" && api.releaseModelRequest) {
        modelOwner.current.requestId = crypto.randomUUID();
        await api.downloadModel({ requestId: modelOwner.current.requestId });
      } else if (action !== "refresh") await api[action]();
      if (!alive.current || operation.current !== current) return;
      const next = SpeechStatus.parse(await api.status());
      if (alive.current && operation.current === current) {
        setStatus(next);
        setNotice(action === "downloadModel" && next.capture === "native" ? next.model.confirmation ? "Android prepared English (United States) for this foreground session. Each Start confirms it again before recording." : next.problem?.message ?? "Android preparation request ended without a readiness confirmation."
          : action === "cancelModelDownload" ? "Model download cancelled." : action === "removeModel" ? "Downloaded model removed from this device." : "Local speech status refreshed.");
      }
    } catch {
      if (alive.current && operation.current === current) {
        try {
          const next = SpeechStatus.parse(await api.status());
          if (alive.current && operation.current === current) { setStatus(next); setProblem(next.problem?.message ?? "The local speech action could not finish. Check this device's storage and network, then try again."); }
        } catch { if (alive.current && operation.current === current) setProblem("The local speech action could not finish. Check this device's storage and network, then try again."); }
      }
    }
    finally { if (alive.current && operation.current === current) { running.current = null; setWorking(null); } }
  }

  const downloading = status?.model.state === "downloading" || status?.model.state === "verifying";
  const selectedLanguage = status?.languages.some((item) => item.tag === language) ? language : status?.languages[0]?.tag ?? "";
  return <SettingsSection title="Voice" note="Dictation runs locally on the device capturing your voice, even when the host PC is offline. Transcripts stay editable until you choose Send.">
    <SettingsCard>
      <SettingsRow title="Local engine"><span className="text-[13px] text-secondary">{status ? status.engine === "whisper.cpp" ? "whisper.cpp on this device" : "Android on-device recognition" : "Unavailable or not checked"}</span></SettingsRow>
      <SettingsRow title="Dictation availability"><span role="status" className="text-[13px] text-secondary">{status ? status.model.confirmation ? "Prepared · Start confirms before recording" : status.available ? "Ready" : "Unavailable" : "Unknown"}</span></SettingsRow>
      <SettingsRow title="Microphone permission" note="Permission is requested only when you start dictation in a conversation."><span className="text-[13px] text-secondary">{status?.permission ?? "Unknown"}</span></SettingsRow>
      <SettingsRow title="Language" note="Only languages reported by the local engine are listed."><select aria-label="Dictation language" className={SELECT} value={selectedLanguage} disabled={!status?.languages.length} onChange={(event) => savePreference("language", event.target.value)}>
        {!status?.languages.length && <option value="">No supported language available</option>}
        {status?.languages.map((item) => <option key={item.tag} value={item.tag}>{item.label}</option>)}
      </select></SettingsRow>
      {status?.capture === "pcm" && <SettingsRow title="Input device" note="Microphone names appear only after access was granted. This list never requests microphone access.">
        <div className="flex max-w-full flex-wrap gap-2"><select aria-label="Dictation microphone" className={SELECT} value={device} onChange={(event) => savePreference("inputDevice", event.target.value)}>
          <option value="">System default</option>
          {device && !devices.some((entry) => entry.deviceId === device) && <option value={device}>Previously selected microphone</option>}
          {devices.map((entry) => <option key={entry.deviceId} value={entry.deviceId}>{entry.label}</option>)}
        </select><Pill className="min-h-11" onClick={() => void microphones()}>Refresh microphones</Pill></div>
      </SettingsRow>}
      {status?.capture === "native" && <SettingsRow title="Input device" note="Android selects the microphone and manages offline language data. An explicit download request never starts recording. Tjommie cannot cancel a system download or remove its data." />}
      <SettingsRow title="Local model" note={status?.model.state === "system-managed" ? "Request English (United States) through Android's on-device speech service. Scheduled does not mean downloaded; leaving these controls detaches observation and the system download may continue." : status?.model.bytes ? `${Math.ceil(status.model.bytes / 1048576)} MiB on this device. Downloading a model does not start recording.` : "No local model information is available."}>
        <span className="text-[13px] text-secondary">{status?.model.state ?? "Unknown"}{downloading && status && ` · ${Math.min(100, Math.floor(status.model.received / Math.max(1, status.model.bytes) * 100))}%`}</span>
      </SettingsRow>
      {status?.model.confirmation && <SettingsRow title="SDK preparation" note="Android confirmed English (United States) for this foreground page. This is separate from the installed-language query; every explicit Start obtains fresh confirmation on its own recognizer."><span className="text-[13px] text-secondary">English (United States)</span></SettingsRow>}
      {status?.problem && <SettingsRow title="On-device status"><span role="status" className="text-[13px] text-secondary">{status.problem.message}</span></SettingsRow>}
    </SettingsCard>
    <div className="mt-4 flex flex-wrap gap-2">
      <Pill className="min-h-11" disabled={!api || working !== null} onClick={() => void run("refresh")}>Refresh voice status</Pill>
      {status?.engine === "android-on-device" && status.capture === "native" && <Pill className="min-h-11" disabled={!api || working !== null || status.permission !== "granted" || status.problem?.code === "on_device_unavailable"} onClick={() => void run("downloadModel")}>{working === "downloadModel" ? "Waiting for Android preparation…" : "Request English (United States) download"}</Pill>}
      {downloading && <Pill className="min-h-11" disabled={working === "cancelModelDownload"} onClick={() => void run("cancelModelDownload")}>Cancel model download</Pill>}
      {status?.capture === "pcm" && !downloading && status.problem?.code !== "runtime_unavailable" && status.model.state !== "ready" && <Pill className="min-h-11" disabled={working !== null} onClick={() => void run("downloadModel")}>Download English model</Pill>}
      {status?.capture === "pcm" && status.model.state === "ready" && <Pill className="min-h-11" disabled={working !== null} onClick={() => void run("removeModel")}>Remove downloaded model</Pill>}
    </div>
    {!api && <p className="mt-4 text-[13px] leading-[20px] text-secondary">Local dictation is unavailable in this app. Typing remains available; there is no cloud fallback.</p>}
    {status && !status.available && <p className="mt-3 text-[13px] leading-[20px] text-secondary">A supported local engine, verified or installed model, and microphone permission are needed. No online recognizer is substituted.</p>}
    {notice && <p role="status" className="mt-3 text-[13px] text-secondary">{notice}</p>}
    {problem && <SettingsProblem>{problem}</SettingsProblem>}
  </SettingsSection>;
}
