"use client";

import { Mic, Square } from "lucide-react";
import type { Dictation } from "@/lib/use-dictation";

export function DictationButton({ voice }: { voice: Dictation }) {
  const listening = voice.phase === "listening";
  return <button type="button" aria-label={listening ? "Stop dictation" : "Start dictation"} title={listening ? "Stop dictation" : "Dictate on this device"}
    disabled={voice.phase === "permission" || voice.phase === "transcribing"} aria-pressed={listening}
    onClick={() => void (listening ? voice.stop() : voice.start())}
    className="flex size-11 shrink-0 items-center justify-center rounded-full text-icon hover:text-text disabled:opacity-50">
    {listening ? <Square className="size-4" aria-hidden /> : <Mic className="size-4" aria-hidden />}
  </button>;
}

export function DictationStatus({ voice }: { voice: Dictation }) {
  const downloading = voice.status?.model.state === "downloading" || voice.status?.model.state === "verifying";
  return <div className="text-[12px] leading-[18px] text-secondary">
    <p role="status" aria-live="polite" className="mt-1 px-3">{voice.message}</p>
    {voice.phase !== "idle" && <button type="button" onClick={() => void voice.cancel()} className="min-h-11 px-3 underline">Cancel dictation</button>}
    {voice.optionsOpen && voice.status && <div className="flex flex-wrap items-center gap-2 px-3 py-1">
      <label>Dictation language <select aria-label="Dictation language" value={voice.language} disabled={voice.phase !== "idle"} onChange={(event) => voice.setLanguage(event.target.value)} className="min-h-11 max-w-[180px] rounded bg-canvas px-1 py-1 text-text">
        {voice.status.languages.map((language) => <option key={language.tag} value={language.tag}>{language.label}</option>)}
      </select></label>
      {voice.devices.length > 0 && <label>Microphone <select aria-label="Dictation microphone" value={voice.deviceId} disabled={voice.phase !== "idle"} onChange={(event) => voice.setDevice(event.target.value)} className="min-h-11 max-w-[180px] rounded bg-canvas px-1 py-1 text-text">
        <option value="">System default</option>{voice.devices.filter((device) => device.deviceId).map((device, index) => <option key={device.deviceId} value={device.deviceId}>{device.label || `Microphone ${index + 1}`}</option>)}
      </select></label>}
      {voice.status.model.state === "system-managed" ? <span>Language data is managed in Android speech settings.</span> : downloading ? <>
        <span>{voice.status.model.state === "verifying" ? "Verifying model…" : `Downloading model: ${Math.floor(voice.status.model.received / Math.max(1, voice.status.model.bytes) * 100)}%`}</span>
        <button type="button" onClick={() => void voice.cancelDownload()} className="min-h-11 underline">Cancel download</button>
      </> : voice.status.model.state !== "ready" && voice.status.problem?.code !== "runtime_unavailable" ? <button type="button" onClick={() => void voice.download()} className="min-h-11 underline">Download English model ({Math.ceil(voice.status.model.bytes / 1048576)} MiB)</button>
        : voice.status.model.state === "ready" ? <button type="button" onClick={() => void voice.removeModel()} className="min-h-11 underline">Remove downloaded model</button> : null}
    </div>}
    {voice.transcript && <div className="mx-3 my-2 rounded border border-divider p-2">
      <p className="whitespace-pre-wrap break-words text-text">{voice.transcript}</p>
      <button type="button" onClick={voice.insert} className="min-h-11 pr-3 underline">Insert transcript</button>
      <button type="button" onClick={voice.discard} className="min-h-11 underline">Discard transcript</button>
    </div>}
  </div>;
}
