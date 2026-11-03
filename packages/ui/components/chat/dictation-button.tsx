"use client";

import { Popover } from "@base-ui/react/popover";
import { Mic, Settings2, Square, X } from "lucide-react";
import type { Dictation } from "@/lib/use-dictation";
import { cn } from "@/lib/utils";

const ACTION = "flex size-11 shrink-0 touch-manipulation select-none items-center justify-center rounded-full text-icon hover:bg-highlight hover:text-text active:bg-highlight disabled:opacity-50";
const MODEL_ACTION = "min-h-11 touch-manipulation select-none rounded-[10px] bg-pill px-3 py-2 text-left text-[14px] text-text hover:bg-highlight active:bg-highlight disabled:opacity-50";
const SELECT = "min-h-11 min-w-0 w-full rounded-[10px] border border-input-stroke bg-canvas px-3 py-2 text-[16px] leading-6 text-text disabled:opacity-50 sm:text-[14px]";

export function DictationButton({ voice }: { voice: Dictation }) {
  const listening = voice.phase === "listening";
  return <button type="button" aria-label={listening ? "Stop dictation" : "Start dictation"} title={listening ? "Stop dictation" : "Dictate on this device"}
    disabled={voice.phase === "permission" || voice.phase === "transcribing" || voice.modelWorking} aria-pressed={listening}
    onClick={() => void (listening ? voice.stop() : voice.start())}
    className={cn(ACTION, listening && "bg-danger/10 text-danger")}>
    {listening ? <Square className="size-[18px]" aria-hidden /> : <Mic className="size-[18px]" aria-hidden />}
  </button>;
}

// Setup is an intentional popover; starting or finishing a recording never opens it.
export function DictationOptions({ voice, text = false }: { voice: Dictation; text?: boolean }) {
  const status = voice.status;
  const downloading = status?.model.state === "downloading" || status?.model.state === "verifying";
  return <Popover.Root>
    <Popover.Trigger type="button" aria-label="Dictation settings" title="Dictation settings"
      className={text ? "min-h-11 shrink-0 touch-manipulation select-none rounded-[10px] px-3 text-[13px] font-medium text-text hover:bg-highlight active:bg-highlight" : ACTION}>
      {text ? "Voice settings" : <Settings2 className="size-[18px]" aria-hidden />}
    </Popover.Trigger>
    <Popover.Portal>
      <Popover.Positioner side="top" align="end" sideOffset={8} className="z-50">
        <Popover.Popup className="max-h-[min(var(--available-height,70dvh),70dvh)] w-[min(320px,calc(100vw-24px))] overflow-y-auto overscroll-contain rounded-[18px] border border-divider bg-popover p-4 shadow-lg">
          <div className="-mr-2 -mt-2 mb-2 flex min-w-0 items-center gap-2">
            <Popover.Title className="min-w-0 flex-1 text-[15px] font-semibold text-title">Dictation settings</Popover.Title>
            <Popover.Close type="button" aria-label="Close dictation settings" className={ACTION}><X className="size-[18px]" aria-hidden /></Popover.Close>
          </div>
          <Popover.Description className="mb-4 text-[13px] leading-5 text-secondary">Voice stays on this device. Nothing is sent until you choose Send.</Popover.Description>
          {voice.issue && <p role="status" className="mb-4 break-words text-[13px] leading-5 text-secondary">{voice.issue}</p>}
          {status?.problem && status.problem.message !== voice.issue && <p role="status" className="mb-4 break-words text-[13px] leading-5 text-secondary">{status.problem.message}</p>}
          {status ? <div className="flex flex-col gap-4">
            <label className="flex min-w-0 flex-col gap-1.5 text-[13px] text-secondary">Language
              <select aria-label="Dictation language" value={voice.language} disabled={voice.phase !== "idle" || !status.languages.length} onChange={(event) => voice.setLanguage(event.target.value)} className={SELECT}>
                {!status.languages.length && <option value="">No confirmed language</option>}
                {status.languages.map((language) => <option key={language.tag} value={language.tag}>{language.label}</option>)}
              </select>
            </label>
            {voice.devices.length > 0 && <label className="flex min-w-0 flex-col gap-1.5 text-[13px] text-secondary">Microphone
              <select aria-label="Dictation microphone" value={voice.deviceId} disabled={voice.phase !== "idle"} onChange={(event) => voice.setDevice(event.target.value)} className={SELECT}>
                <option value="">System default</option>{voice.devices.filter((device) => device.deviceId).map((device, index) => <option key={device.deviceId} value={device.deviceId}>{device.label || `Microphone ${index + 1}`}</option>)}
              </select>
            </label>}
            {status.model.state === "system-managed" ? <>
              <p className="text-[13px] leading-5 text-secondary">Android manages offline language data. Preparation does not record audio; a system download may continue after you leave.</p>
              {status.model.confirmation && <p className="text-[13px] leading-5 text-secondary">English (United States) is prepared for this foreground session. Each Start confirms it again.</p>}
              <button type="button" disabled={voice.phase !== "idle" || voice.modelWorking || status.permission !== "granted" || status.problem?.code === "on_device_unavailable"} onClick={() => void voice.download()} className={MODEL_ACTION}>
                {voice.modelWorking ? "Waiting for Android preparation…" : "Request English (United States) download"}
              </button>
            </> : downloading ? <>
              <p role="status" className="text-[13px] leading-5 text-secondary">{status.model.state === "verifying" ? "Verifying model…" : `Downloading model: ${Math.floor(status.model.received / Math.max(1, status.model.bytes) * 100)}%`}</p>
              <button type="button" onClick={() => void voice.cancelDownload()} className={MODEL_ACTION}>Cancel download</button>
            </> : status.model.state !== "ready" && status.problem?.code !== "runtime_unavailable" ? <button type="button" disabled={voice.phase !== "idle" || voice.modelWorking} onClick={() => void voice.download()} className={MODEL_ACTION}>Download English model ({Math.ceil(status.model.bytes / 1048576)} MiB)</button>
              : status.model.state === "ready" ? <button type="button" disabled={voice.phase !== "idle" || voice.modelWorking} onClick={() => void voice.removeModel()} className={MODEL_ACTION}>Remove downloaded model</button> : null}
          </div> : <p className="text-[13px] leading-5 text-secondary">Local dictation is available in supported desktop and Android apps. You can keep typing.</p>}
        </Popover.Popup>
      </Popover.Positioner>
    </Popover.Portal>
  </Popover.Root>;
}

export function DictationStatus({ voice }: { voice: Dictation }) {
  const active = voice.phase !== "idle";
  if (!active && !voice.modelWorking && !voice.issue && !voice.transcript) return null;
  const label = voice.phase === "permission" ? "Preparing microphone…" : voice.phase === "listening" ? "Listening…" : voice.phase === "transcribing" ? "Transcribing…" : "Preparing voice…";
  return <div className="mb-2 min-w-0 rounded-[14px] border border-divider bg-card px-3 text-[13px] leading-5 text-secondary">
    {(active || voice.modelWorking) && <div className="flex min-h-11 min-w-0 items-center gap-2" role="status" aria-live="polite">
      <span className={cn("size-1.5 shrink-0 rounded-full", voice.phase === "listening" ? "bg-danger" : "bg-accent")} aria-hidden />
      <span className="min-w-0 flex-1 break-words">{label}</span>
      {active && <button type="button" onClick={() => void voice.cancel()} className="min-h-11 touch-manipulation select-none rounded-[10px] px-2 font-medium text-text hover:bg-highlight active:bg-highlight">Cancel dictation</button>}
    </div>}
    {voice.issue && !active && <div className="flex min-w-0 flex-wrap items-center justify-between gap-x-2" role="status"><span>Dictation needs attention.</span><DictationOptions voice={voice} text /></div>}
    {voice.transcript && <div className="py-3">
      <p className="mb-2 font-medium text-text">Transcript ready to insert</p>
      <p tabIndex={0} role="region" aria-label="Transcript to review before insertion" className="max-h-32 overflow-y-auto whitespace-pre-wrap break-words text-text">{voice.transcript}</p>
      <div className="mt-2 flex flex-wrap gap-2">
        <button type="button" onClick={voice.insert} className={MODEL_ACTION}>Insert transcript</button>
        <button type="button" onClick={voice.discard} className={MODEL_ACTION}>Discard transcript</button>
      </div>
    </div>}
  </div>;
}
