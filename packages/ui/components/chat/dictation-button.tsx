"use client";

import { Popover } from "@base-ui/react/popover";
import { Menu } from "@base-ui/react/menu";
import { useId } from "react";
import { Check, ChevronDown, Mic, Settings2, Square, X } from "lucide-react";
import { Label } from "@/components/controls";
import { IconButton } from "@/components/icon-button";
import { MENU_ITEM, MENU_POPUP } from "@/components/menu";
import { Pill } from "@/components/pill";
import type { Dictation } from "@/lib/use-dictation";
import { cn } from "@/lib/utils";

export function DictationButton({ voice }: { voice: Dictation }) {
  const listening = voice.phase === "listening";
  return <IconButton label={listening ? "Stop dictation" : "Start dictation"} title={listening ? "Stop dictation" : "Dictate on this device"}
    disabled={voice.phase === "permission" || voice.phase === "transcribing" || voice.modelWorking} aria-pressed={listening}
    onClick={() => void (listening ? voice.stop() : voice.start())}
    className={cn("rounded-full disabled:opacity-50 max-sm:size-11", listening && "text-danger")}>
    {listening ? <Square aria-hidden /> : <Mic aria-hidden />}
  </IconButton>;
}

// Setup is an intentional popover; starting or finishing a recording never opens it.
export function DictationOptions({ voice, text = false }: { voice: Dictation; text?: boolean }) {
  const id = useId();
  const status = voice.status;
  const downloading = status?.model.state === "downloading" || status?.model.state === "verifying";
  return <Popover.Root>
    <Popover.Trigger aria-label="Dictation settings" title="Dictation settings" render={text ? <Pill className="max-sm:min-h-11" /> : <IconButton label="Dictation settings" className="rounded-full max-sm:size-11" />}>
      {text ? "Voice settings" : <Settings2 aria-hidden />}
    </Popover.Trigger>
    <Popover.Portal>
      <Popover.Positioner side="top" align="end" sideOffset={8} className="z-50">
        <Popover.Popup className={cn(MENU_POPUP, "max-h-[min(var(--available-height,70dvh),70dvh)] w-[min(320px,calc(100vw-32px))] min-w-0 overflow-y-auto overscroll-contain p-4")}>
          <div className="-mr-2 -mt-2 mb-2 flex min-w-0 items-center gap-2">
            <Popover.Title className="min-w-0 flex-1 text-[15px] font-semibold text-title">Dictation settings</Popover.Title>
            <Popover.Close render={<IconButton label="Close dictation settings" className="max-sm:size-11" />}><X aria-hidden /></Popover.Close>
          </div>
          <Popover.Description className="mb-4 text-[13px] leading-5 text-secondary">Voice stays on this device. Nothing is sent until you choose Send.</Popover.Description>
          {voice.issue && <p role="status" className="mb-4 break-words text-[13px] leading-5 text-secondary">{voice.issue}</p>}
          {status?.problem && status.problem.message !== voice.issue && <p role="status" className="mb-4 break-words text-[13px] leading-5 text-secondary">{status.problem.message}</p>}
          {status ? <div className="flex flex-col gap-4">
            <div className="min-w-0">
              <Label htmlFor={`${id}-language`}>Language</Label>
              <Menu.Root>
                <Menu.Trigger id={`${id}-language`} aria-label="Dictation language" disabled={voice.phase !== "idle" || !status.languages.length} render={<Pill className="mt-1.5 max-w-full max-sm:min-h-11" />}>
                  <span className="min-w-0 truncate">{status.languages.find((language) => language.tag === voice.language)?.label ?? "No confirmed language"}</span><ChevronDown className="size-4 shrink-0" aria-hidden />
                </Menu.Trigger>
                <Menu.Portal><Menu.Positioner sideOffset={6} align="start" className="z-50"><Menu.Popup className={cn(MENU_POPUP, "max-h-[var(--available-height)] max-w-[calc(100vw-32px)] overflow-y-auto")}>
                  <Menu.RadioGroup value={voice.language} onValueChange={(value: unknown) => { if (typeof value === "string") voice.setLanguage(value); }}>
                    {status.languages.map((language) => <Menu.RadioItem key={language.tag} value={language.tag} closeOnClick className={MENU_ITEM}><span className="min-w-0 flex-1 truncate">{language.label}</span><Menu.RadioItemIndicator><Check aria-hidden /></Menu.RadioItemIndicator></Menu.RadioItem>)}
                  </Menu.RadioGroup>
                </Menu.Popup></Menu.Positioner></Menu.Portal>
              </Menu.Root>
            </div>
            {voice.devices.length > 0 && <div className="min-w-0">
              <Label htmlFor={`${id}-microphone`}>Microphone</Label>
              <Menu.Root>
                <Menu.Trigger id={`${id}-microphone`} aria-label="Dictation microphone" disabled={voice.phase !== "idle"} render={<Pill className="mt-1.5 max-w-full max-sm:min-h-11" />}>
                  <span className="min-w-0 truncate">{voice.deviceId ? voice.devices.find((device) => device.deviceId === voice.deviceId)?.label || "Selected microphone" : "System default"}</span><ChevronDown className="size-4 shrink-0" aria-hidden />
                </Menu.Trigger>
                <Menu.Portal><Menu.Positioner sideOffset={6} align="start" className="z-50"><Menu.Popup className={cn(MENU_POPUP, "max-h-[var(--available-height)] max-w-[calc(100vw-32px)] overflow-y-auto")}>
                  <Menu.RadioGroup value={voice.deviceId} onValueChange={(value: unknown) => { if (typeof value === "string") voice.setDevice(value); }}>
                    <Menu.RadioItem value="" closeOnClick className={MENU_ITEM}><span className="flex-1">System default</span><Menu.RadioItemIndicator><Check aria-hidden /></Menu.RadioItemIndicator></Menu.RadioItem>
                    {voice.devices.filter((device) => device.deviceId).map((device, index) => <Menu.RadioItem key={device.deviceId} value={device.deviceId} closeOnClick className={MENU_ITEM}><span className="min-w-0 flex-1 truncate">{device.label || `Microphone ${index + 1}`}</span><Menu.RadioItemIndicator><Check aria-hidden /></Menu.RadioItemIndicator></Menu.RadioItem>)}
                  </Menu.RadioGroup>
                </Menu.Popup></Menu.Positioner></Menu.Portal>
              </Menu.Root>
            </div>}
            {status.model.state === "system-managed" ? <>
              <p className="text-[13px] leading-5 text-secondary">Android manages offline language data. Preparation does not record audio; a system download may continue after you leave.</p>
              {status.model.confirmation && <p className="text-[13px] leading-5 text-secondary">English (United States) is prepared for this foreground session. Each Start confirms it again.</p>}
              <Pill disabled={voice.phase !== "idle" || voice.modelWorking || status.permission !== "granted" || status.problem?.code === "on_device_unavailable"} onClick={() => void voice.download()} className="h-auto! min-h-8 max-w-full whitespace-normal! py-2 max-sm:min-h-11">
                {voice.modelWorking ? "Waiting for Android preparation…" : "Request English (United States) download"}
              </Pill>
            </> : downloading ? <>
              <p role="status" className="text-[13px] leading-5 text-secondary">{status.model.state === "verifying" ? "Verifying model…" : `Downloading model: ${Math.floor(status.model.received / Math.max(1, status.model.bytes) * 100)}%`}</p>
              <Pill onClick={() => void voice.cancelDownload()} className="max-sm:min-h-11">Cancel download</Pill>
            </> : status.model.state !== "ready" && status.problem?.code !== "runtime_unavailable" ? <Pill disabled={voice.phase !== "idle" || voice.modelWorking} onClick={() => void voice.download()} className="h-auto! min-h-8 max-w-full whitespace-normal! py-2 max-sm:min-h-11">Download English model ({Math.ceil(status.model.bytes / 1048576)} MiB)</Pill>
              : status.model.state === "ready" ? <Pill disabled={voice.phase !== "idle" || voice.modelWorking} onClick={() => void voice.removeModel()} className="max-sm:min-h-11">Remove downloaded model</Pill> : null}
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
  return <div className="mb-2 min-w-0 rounded-[12px] bg-card px-3 text-[13px] leading-[18px] text-secondary">
    {(active || voice.modelWorking) && <div className="flex min-h-11 min-w-0 items-center gap-2" role="status" aria-live="polite">
      <span className={cn("size-1.5 shrink-0 rounded-full", voice.phase === "listening" ? "bg-danger" : "bg-accent")} aria-hidden />
      <span className="min-w-0 flex-1 break-words">{label}</span>
      {active && <Pill onClick={() => void voice.cancel()} className="max-sm:min-h-11">Cancel dictation</Pill>}
    </div>}
    {voice.issue && !active && <div className="flex min-w-0 flex-wrap items-center justify-between gap-x-2" role="status"><span>Dictation needs attention.</span><DictationOptions voice={voice} text /></div>}
    {voice.transcript && <div className="py-3">
      <p className="mb-2 font-medium text-text">Transcript ready to insert</p>
      <p tabIndex={0} role="region" aria-label="Transcript to review before insertion" className="max-h-32 overflow-y-auto whitespace-pre-wrap break-words text-text">{voice.transcript}</p>
      <div className="mt-2 flex flex-wrap gap-2">
        <Pill primary onClick={voice.insert} className="max-sm:min-h-11">Insert transcript</Pill>
        <Pill onClick={voice.discard} className="max-sm:min-h-11">Discard transcript</Pill>
      </div>
    </div>}
  </div>;
}
