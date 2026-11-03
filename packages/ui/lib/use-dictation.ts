"use client";

import { useEffect, useLayoutEffect, useRef, useState, type RefObject } from "react";
import { SpeechEvent, SpeechStatus, type SpeechApi } from "../../protocol/src/speech.ts";
import { inputEdit, insertTranscript, moveRange, type DraftEdit, type InsertionRange } from "./dictation";
import { capturePcm, claimSpeech, localSpeech, type PcmCapture } from "./local-speech";
import { getState, useApp } from "./store";

type Phase = "idle" | "permission" | "listening" | "transcribing";
type Active = { id: string; scope: string; host: string | null; phase: Phase; range: InsertionRange | null; capture?: PcmCapture; release?: () => void; cleanup?: Promise<void>; timer?: ReturnType<typeof setTimeout> };
type Field = HTMLInputElement | HTMLTextAreaElement;

function preference(key: string): string { try { return localStorage.getItem(`tjommie.voice.${key}`) ?? ""; } catch { return ""; } }
function savePreference(key: string, value: string): void { try { localStorage.setItem(`tjommie.voice.${key}`, value); } catch { /* Dictation remains usable without persistent preferences. */ } }

export function useDictation(kind: "chat" | "room" | "exchange", target: string, field: RefObject<Field | null>, text: string, setText: (text: string) => void) {
  const host = useApp((state) => state.host);
  const scope = JSON.stringify([host, kind, target]);
  const latest = useRef({ text, setText, scope });
  useLayoutEffect(() => { latest.current = { text, setText, scope }; }, [text, setText, scope]);
  const mounted = useRef(false);
  const active = useRef<Active | null>(null);
  const api = useRef<SpeechApi | null>(null);
  const edit = useRef<DraftEdit | null>(null);
  const draftRevision = useRef(0);
  const intent = useRef(0);
  const cancelling = useRef<Promise<void> | null>(null);
  const [status, setStatus] = useState<SpeechStatus | null>(null);
  const [phase, setPhase] = useState<Phase>("idle");
  const [sessionScope, setSessionScope] = useState("");
  const [message, setMessage] = useState("");
  const [pending, setPending] = useState({ scope: "", text: "" });
  const transcript = pending.scope === scope ? pending.text : "";
  const setTranscript = (value: string): void => { setPending({ scope: latest.current.scope, text: value }); };
  const [language, selectLanguage] = useState("");
  const [deviceId, selectDevice] = useState(() => preference("inputDevice"));
  const [devices, setDevices] = useState<MediaDeviceInfo[]>([]);
  const [optionsOpen, setOptionsOpen] = useState(false);

  function showStatus(value: SpeechStatus): void {
    setStatus(value);
    selectLanguage((current) => value.languages.some((item) => item.tag === current) ? current : value.languages.find((item) => item.tag === preference("language"))?.tag ?? value.languages[0]?.tag ?? "");
  }

  const owns = (session: Active): boolean => mounted.current && active.current === session && latest.current.scope === session.scope && getState().host === session.host;

  async function cancelActive(): Promise<void> {
    const session = active.current;
    active.current = null;
    if (mounted.current) { setPhase("idle"); setTranscript(""); }
    if (!session) { await cancelling.current; return; }
    clearTimeout(session.timer);
    const bridge = api.current;
    session.cleanup = (async () => {
      let failed = false;
      try { await session.capture?.cancel(); } catch { failed = true; }
      try { await bridge?.cancel({ sessionId: session.id }); } catch { failed = true; }
      session.release?.();
      if (failed && mounted.current) setMessage("Local speech cleanup could not finish. Check storage and reopen the app before dictating again.");
    })();
    cancelling.current = session.cleanup;
    try { await session.cleanup; } finally { if (cancelling.current === session.cleanup) cancelling.current = null; }
  }

  function cancel(): Promise<void> { intent.current += 1; return cancelActive(); }

  function commit(value: string, range: InsertionRange): void {
    const next = insertTranscript(latest.current.text, range, value);
    latest.current.text = next.text;
    latest.current.setText(next.text);
    requestAnimationFrame(() => { field.current?.focus(); field.current?.setSelectionRange(next.caret, next.caret); });
  }

  async function stop(): Promise<void> {
    const session = active.current;
    if (!session || !owns(session) || session.phase !== "listening") return;
    session.phase = "transcribing"; clearTimeout(session.timer);
    setPhase("transcribing"); setMessage("Transcribing on this device…");
    try {
      await session.capture?.finish();
      session.capture = undefined;
      if (owns(session)) await api.current?.stop({ sessionId: session.id });
    } catch {
      if (owns(session)) { await cancel(); setMessage("Couldn’t finish local dictation. Your draft is unchanged; try again."); }
    }
  }

  useEffect(() => {
    mounted.current = true;
    const bridge = localSpeech();
    api.current = bridge;
    const remove = bridge?.onEvent((input) => {
      const parsed = SpeechEvent.safeParse(input);
      if (!mounted.current || !parsed.success) return;
      const event = parsed.data;
      if (event.kind === "status") { showStatus(event.status); return; }
      const session = active.current;
      if (!session || event.sessionId !== session.id || !owns(session)) return;
      if (event.kind === "final") {
        if (session.phase !== "transcribing") return;
        active.current = null; clearTimeout(session.timer); session.release?.();
        void session.capture?.cancel();
        setPhase("idle");
        if (session.range) { commit(event.text, session.range); setMessage("Transcript added to your draft. Review it before sending."); }
        else { setTranscript(event.text); setMessage("Your draft changed at the insertion point. Choose where to insert this transcript, or discard it."); }
      } else if (event.phase === "transcribing") {
        if (session.phase !== "transcribing") { session.phase = "transcribing"; void session.capture?.cancel(); session.capture = undefined; }
        clearTimeout(session.timer); setPhase("transcribing"); setMessage("Transcribing on this device…");
      } else if (event.phase === "error" || event.phase === "cancelled") {
        void cancel(); setMessage(event.problem?.message ?? "Dictation cancelled.");
      }
    });
    void bridge?.status().then((input) => { if (mounted.current) showStatus(SpeechStatus.parse(input)); }).catch(() => { if (mounted.current) setMessage("Local speech is unavailable in this app build. You can keep typing."); });
    const hidden = (): void => { if (document.visibilityState === "hidden") void cancel(); };
    const page = (): void => { void cancel(); };
    const blur = (): void => { if (active.current?.phase !== "permission") void cancel(); };
    const element = field.current;
    const beforeInput = (event: Event): void => {
      if (!element) return;
      draftRevision.current += 1;
      edit.current = inputEdit(element, event as InputEvent);
      const session = active.current;
      if (session?.range && (!edit.current || (edit.current.end > session.range.start && edit.current.start < session.range.end))) session.range = null;
    };
    element?.addEventListener("beforeinput", beforeInput);
    document.addEventListener("visibilitychange", hidden);
    window.addEventListener("pagehide", page);
    window.addEventListener("blur", blur);
    return () => {
      mounted.current = false;
      void cancel(); remove?.();
      document.removeEventListener("visibilitychange", hidden);
      window.removeEventListener("pagehide", page); window.removeEventListener("blur", blur);
      element?.removeEventListener("beforeinput", beforeInput);
    };
    // The refs keep typing/caret changes out of the native subscription lifecycle.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [scope]);

  async function start(): Promise<void> {
    setOptionsOpen(true); setMessage(""); setTranscript("");
    const bridge = api.current;
    if (!bridge) { setMessage("Local dictation is available in supported desktop and Android app builds. You can keep typing here."); return; }
    const request = ++intent.current;
    const initiatingScope = latest.current.scope;
    const initiatingHost = getState().host;
    const initiatingRevision = draftRevision.current;
    const initiatingText = latest.current.text;
    const element = field.current;
    const range = { start: element?.selectionStart ?? initiatingText.length, end: element?.selectionEnd ?? initiatingText.length };
    await cancelActive();
    if (!mounted.current || intent.current !== request || latest.current.scope !== initiatingScope || getState().host !== initiatingHost) return;
    const session: Active = { id: crypto.randomUUID(), scope: initiatingScope, host: initiatingHost, phase: "permission", range: draftRevision.current === initiatingRevision && latest.current.text === initiatingText ? range : null };
    active.current = session; setSessionScope(session.scope); setPhase("permission");
    let attemptedNative = false;
    try {
      session.release = await claimSpeech(async () => { if (active.current === session) await cancel(); else await session.cleanup; });
      if (!owns(session)) { session.release(); return; }
      const available = SpeechStatus.parse(await bridge.status());
      if (!owns(session)) return;
      showStatus(available);
      const permissionRequired = available.engine === "android-on-device" && available.capture === "native" && available.model.state === "system-managed"
        && !available.available && available.languages.length === 0 && available.problem?.code === "permission_required" && (available.permission === "prompt" || available.permission === "unknown");
      const saved = preference("language");
      const desired = saved.length <= 40 && /^en(?:-[A-Za-z0-9]{2,8})*$/.test(saved) ? saved : "en-US";
      // This intent only opens the native permission path; native support must confirm it before capture.
      const selected = available.languages.find((item) => item.tag === language)?.tag ?? available.languages[0]?.tag ?? (permissionRequired ? desired : undefined);
      if ((!available.available && !permissionRequired) || !selected) { await cancel(); setMessage(available.problem?.message ?? "No installed local language is available. Check system speech settings."); return; }
      attemptedNative = available.engine === "android-on-device" && available.capture === "native";
      await bridge.start({ sessionId: session.id, language: selected });
      if (!owns(session) || session.phase !== "permission") return;
      if (available.capture === "pcm") {
        const capture = await capturePcm({ api: bridge, sessionId: session.id, deviceId: deviceId || undefined,
          current: () => owns(session) && session.phase === "permission" && document.visibilityState !== "hidden" && document.hasFocus(),
          failed: () => { if (owns(session)) { void cancel(); setMessage("Microphone data could not be processed within its limits. Try again."); } },
          limit: () => { if (owns(session)) void stop(); },
        });
        if (!capture) { if (owns(session)) await cancel(); return; }
        session.capture = capture;
        if (!owns(session)) { await capture.cancel(); return; }
        void navigator.mediaDevices.enumerateDevices().then((items) => { if (mounted.current) setDevices(items.filter((item) => item.kind === "audioinput")); }).catch(() => {});
      }
      session.phase = "listening"; setPhase("listening"); setMessage("Listening on this device. Tap Stop when finished.");
      session.timer = setTimeout(() => { if (owns(session)) void stop(); }, 59750);
    } catch {
      if (owns(session)) { await cancelActive(); setMessage("Couldn’t start local dictation. Check microphone permission and installed speech support, then try again."); }
    } finally {
      if (attemptedNative && mounted.current && intent.current === request && latest.current.scope === initiatingScope && getState().host === initiatingHost) {
        try {
          const current = SpeechStatus.parse(await bridge.status());
          if (mounted.current && intent.current === request && latest.current.scope === initiatingScope && getState().host === initiatingHost) showStatus(current);
        } catch { /* Start/cancellation retains its own result if status refresh is unavailable. */ }
      }
    }
  }

  function changed(next: string): void {
    draftRevision.current += 1;
    const session = active.current;
    if (session) session.range = moveRange(session.range, edit.current, latest.current.text, next);
    edit.current = null;
    latest.current.text = next;
  }

  async function model(action: "downloadModel" | "cancelModelDownload" | "removeModel"): Promise<void> {
    try {
      await api.current?.[action]();
      if (api.current && mounted.current) setStatus(SpeechStatus.parse(await api.current.status()));
    } catch { if (mounted.current) setMessage("Couldn’t complete the speech model action. Check storage and network, then try again."); }
  }

  return {
    status, phase: sessionScope === scope ? phase : "idle" as Phase, message, transcript, language, deviceId, devices, optionsOpen,
    start, stop, cancel, changed,
    setLanguage: (value: string): void => { selectLanguage(value); savePreference("language", value); },
    setDevice: (value: string): void => { selectDevice(value); savePreference("inputDevice", value); },
    download: (): Promise<void> => model("downloadModel"), cancelDownload: (): Promise<void> => model("cancelModelDownload"), removeModel: (): Promise<void> => model("removeModel"),
    insert: (): void => { if (transcript && getState().host === host && pending.scope === latest.current.scope) { const element = field.current; commit(transcript, { start: element?.selectionStart ?? latest.current.text.length, end: element?.selectionEnd ?? latest.current.text.length }); setTranscript(""); setMessage("Transcript added. Review it before sending."); } },
    discard: (): void => { setTranscript(""); setMessage("Transcript discarded."); },
  };
}

export type Dictation = ReturnType<typeof useDictation>;
