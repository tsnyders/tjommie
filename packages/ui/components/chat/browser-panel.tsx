"use client";

import { useEffect, useRef, useState, type ClipboardEvent, type KeyboardEvent, type PointerEvent, type ReactNode } from "react";
import { ArrowLeft, ArrowRight, CornerDownLeft, Loader, Maximize2, Minimize2, Plus, RotateCw, X } from "lucide-react";
import type { Bot, BrowserFrame, BrowserInput } from "@tjommie/protocol";
import { BrowserSession, type BrowserListen, type BrowserView } from "@/lib/browser-session";
import { daemon, request } from "@/lib/connection";
import { useApp } from "@/lib/store";
import { desktop, type NativeBrowserApi } from "@/lib/desktop";
import { NativeBrowserSession, nativeView } from "@/lib/native-browser-session";
import { coveredByAppOverlay, slotBounds } from "@/lib/native-overlay";
import { addressToUrl, framePoint, framePointClamped, frameViewport, googleSignInRejected, imageReady, keyRoute, mouseButton, pictureKey, resizeWorthIt, viewportSize, wheelPixels, type Size } from "@/lib/browser-geometry";
import { cn } from "@/lib/utils";
import { IconButton } from "../icon-button";
import { FIELD } from "../controls";
import { Pill } from "../pill";
import { BrowserLogins } from "./browser-logins";

// The toolbar's calls. (Input, release, resize and drawn are for the picture-based fallback viewport; the native page needs none.)
export type BrowserControls = {
  drawn?: (frame: BrowserFrame) => void; // the picture is on screen, so the live stream may send the next
  input: (input: BrowserInput) => void;
  releaseAll: () => void;
  setControl: (control: "human" | "bot") => void;
  navigate: (url: string) => void;
  history: (action: "back" | "forward" | "reload") => void;
  resize: (width: number, height: number) => void;
  dialog: (accept: boolean, promptText?: string) => void;
  tab: (action: "new" | "select" | "close", pageId?: string) => void;
};

const BUTTON = "press h-8 shrink-0 rounded-[10px] bg-pill px-3 text-[13px] leading-[18px] text-text hover:bg-selected disabled:opacity-50";
const WAIT_LABEL_MS = 800; // a frame that takes longer than this is worth saying so

// The bot's own browser, the Browser pane of the right panel. The page is a native view drawn and driven by the desktop app
// itself: this panel is its toolbar and the place it sits (the viewport below), and the user clicks and types in the page
// directly. The pane stays mounted while Details shows, so tabs and the address draft survive the switch; leaving it hides the
// view and hands the browser back to the bot (a page dialog dismissed), so coming back means Take control again. Unmounting
// (hiding the panel) closes the session, which hands it back too. The bot's browser itself lives on in the desktop app.
export function BrowserPanel({ bot, active, expanded, canExpand = true, onToggleExpanded }: { bot: Bot; active: boolean; expanded: boolean; canExpand?: boolean; onToggleExpanded: () => void }) {
  const host = useApp((state) => state.host);
  const ready = useApp((state) => state.connection.status === "ready");
  const api = host === null ? desktop()?.nativeBrowser : undefined;
  if (!api && !ready) return <p role="status" className="px-4 text-[13px] leading-[18px] text-secondary">The browser host is offline. Reconnect to watch or take control.</p>;
  if (!api) return <RemotePane key={`${host}:${bot.id}`} bot={bot} active={active} expanded={expanded} canExpand={canExpand} onToggleExpanded={onToggleExpanded} />;
  return <NativePane key={bot.id} bot={bot} api={api} active={active} expanded={expanded} canExpand={canExpand} onToggleExpanded={onToggleExpanded} />;
}

// The live stream's frames on the connection the remote pane's calls go to. Not connected: none, and browser.stream fails too.
const streamFrames: BrowserListen = (listener) => {
  try {
    return daemon().subscribe((event) => {
      if (event.event === "browser.streamFrame") listener(event.data);
    });
  } catch {
    return () => {};
  }
};

function RemotePane({ bot, active, expanded, canExpand, onToggleExpanded }: { bot: Bot; active: boolean; expanded: boolean; canExpand: boolean; onToggleExpanded: () => void }) {
  const ready = useApp((state) => state.connection.status === "ready");
  const [view, setView] = useState<BrowserView>({ state: null, frame: null, error: null });
  const [text, setText] = useState("");
  const [session] = useState(() => new BrowserSession(bot.id, request, setView, streamFrames));
  useEffect(() => { if (ready) session.open(); return () => { void session.close(); }; }, [session, ready]);
  useEffect(() => {
    const visibility = (): void => session.setVisible(active && ready && document.visibilityState !== "hidden");
    visibility(); document.addEventListener("visibilitychange", visibility);
    return () => { document.removeEventListener("visibilitychange", visibility); session.setVisible(false); };
  }, [active, ready, session]);
  if (!ready) return <p role="status" className="px-4 text-[13px] leading-[18px] text-secondary">The browser host is offline. Reconnect to watch or take control.</p>;
  const canType = view.state?.canControl === true && !view.state.privateActive && !!view.frame && !view.busy;
  return <div className="flex min-h-0 flex-1 flex-col">
    <BrowserChrome view={view} botName={bot.name} controls={session} active={active} expanded={expanded} canExpand={canExpand} onToggleExpanded={onToggleExpanded} onRetry={() => session.open()} />
    {active && !view.state?.privateActive && <form className="flex items-center gap-2 px-4 pb-4" onSubmit={(event) => { event.preventDefault(); if (canType && text) session.input({ kind: "text", text }); }}>
      <input aria-label="Text to type into the focused browser field" value={text} onChange={(event) => setText(event.target.value)} disabled={!canType} autoComplete="off" className={cn(FIELD, "h-11 min-w-0 flex-1 disabled:opacity-50")} />
      <Pill type="submit" disabled={!canType || !text} className="max-sm:min-h-11">Type</Pill>
    </form>}
  </div>;
}

// Shift+Tab out of the page lands on the control strip's button, where the keyboard would have gone.
const focusControl = (): void => document.querySelector<HTMLElement>('[data-focus-return="control"]')?.focus();

function NativePane({ bot, api, active, expanded, canExpand, onToggleExpanded }: { bot: Bot; api: NativeBrowserApi; active: boolean; expanded: boolean; canExpand: boolean; onToggleExpanded: () => void }) {
  const [view, setView] = useState<BrowserView>(nativeView(null, null));
  const [session] = useState(() => new NativeBrowserSession(bot.id, api, setView, focusControl));
  useEffect(() => {
    session.open();
    return () => {
      void session.close();
    };
  }, [session]);
  const wasActive = useRef(active);
  useEffect(() => {
    if (wasActive.current && !active) void session.suspend();
    wasActive.current = active;
  }, [active, session]);
  // Typing and clicking go to the page directly, so the controls here are only the toolbar's.
  const controls: BrowserControls = {
    input: () => {},
    releaseAll: () => {},
    resize: () => {},
    setControl: (control) => session.setControl(control),
    navigate: (url) => session.navigate(url),
    history: (action) => session.history(action),
    dialog: (accept, text) => session.dialog(accept, text),
    tab: (action, pageId) => session.tab(action, pageId),
  };
  return (
    <BrowserChrome
      view={view}
      botName={bot.name}
      controls={controls}
      active={active}
      expanded={expanded}
      canExpand={canExpand}
      onToggleExpanded={onToggleExpanded}
      onRetry={() => session.open()}
      viewport={<NativeViewport session={session} active={active} />}
    />
  );
}

// The place the native page sits. The page is a view over the window, so this reports where the slot is, every frame while it
// is shown (the panel moves with a drag, a resize, an expand or a scroll, and none of those is an event of the slot), and
// leaves it when the pane or window is hidden, and temporarily covers it for app menus and dialogs without handing control back.
function NativeViewport({ session, active }: { session: NativeBrowserSession; active: boolean }) {
  const slot = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!active) {
      session.setBounds(null);
      return;
    }
    // Every frame the slot is read, but the desktop app is only told when the bounds differ from the last ones sent.
    let frame = 0;
    const follow = (): void => {
      session.setBounds(slotBounds(slot.current?.getBoundingClientRect(), document.visibilityState !== "visible"), coveredByAppOverlay(document));
      frame = requestAnimationFrame(follow);
    };
    follow();
    return () => {
      cancelAnimationFrame(frame);
      session.setBounds(null);
    };
  }, [session, active]);
  return <div ref={slot} role="application" aria-label="Browser viewport" tabIndex={-1} className="relative min-h-[240px] w-full flex-1 overflow-hidden rounded-[12px] bg-card ring-1 ring-divider" />;
}

export function BrowserChrome({
  view,
  botName,
  controls,
  expanded,
  canExpand = true,
  onToggleExpanded,
  onRetry,
  viewport,
  active = true,
}: {
  view: BrowserView;
  botName: string;
  controls: BrowserControls;
  expanded: boolean;
  canExpand?: boolean; // the sheet on a phone is already the whole screen
  onToggleExpanded: () => void;
  onRetry: () => void;
  viewport?: ReactNode; // the native page's place; without it, the picture-based viewport
  active?: boolean;
}) {
  const { state, error } = view;
  const human = state?.control === "human" && state.canControl !== false && !state.privateActive;
  const tab = state?.tabs.find((each) => each.id === state.activePageId);
  const [draft, setDraft] = useState<string | null>(null);
  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col gap-2 overflow-y-auto px-4 pb-4">
      {state && (
        <>
          <div className="flex min-w-0 shrink-0 flex-wrap items-center gap-1">
          <div role="tablist" aria-label="Browser tabs" className="flex min-w-0 flex-1 items-center gap-1 overflow-x-auto">
            {state.tabs.map((each) => {
              const title = each.title || each.url || "New tab";
              return (
                <div key={each.id} className={cn("flex h-9 w-48 shrink-0 items-center rounded-[10px] max-sm:h-11", each.id === state.activePageId ? "bg-selected" : "bg-card")}>
                  <button
                    type="button"
                    role="tab"
                    aria-selected={each.id === state.activePageId}
                    aria-label={`Browser tab ${title}`}
                    title={title}
                    disabled={!human}
                    onClick={() => controls.tab("select", each.id)}
                    className="h-full min-w-0 flex-1 truncate pl-3 pr-1 text-left text-[13px] leading-[18px] text-text disabled:opacity-70"
                  >
                    {title}
                  </button>
                  <button type="button" aria-label={`Close tab ${title}`} disabled={!human} onClick={() => controls.tab("close", each.id)} className="press mr-1 flex size-7 shrink-0 items-center justify-center rounded-[8px] text-icon hover:bg-pill disabled:opacity-50 max-sm:size-11 max-sm:mr-0">
                    <X className="size-3.5" aria-hidden />
                  </button>
                </div>
              );
            })}
          </div>
            <IconButton label="New tab" className="size-9 max-sm:size-11" disabled={!human} onClick={() => controls.tab("new")}>
              <Plus aria-hidden />
            </IconButton>
            {canExpand && <IconButton label={expanded ? "Restore browser" : "Expand browser"} className="size-9 max-sm:size-11" data-focus-return="expand" onClick={onToggleExpanded}>
              {expanded ? <Minimize2 aria-hidden /> : <Maximize2 aria-hidden />}
            </IconButton>}
            {active && <BrowserLogins state={state} onRefresh={onRetry} />}
          </div>
          <form
            className="flex min-w-0 shrink-0 items-center gap-1"
            onSubmit={(event) => {
              event.preventDefault();
              // The form's own value, so a blur that cleared the draft just before this click can't lose the address.
              const typed = new FormData(event.currentTarget).get("address");
              if (typeof typed === "string" && typed.trim()) controls.navigate(addressToUrl(typed));
              setDraft(null);
            }}
          >
            <IconButton label="Back" className="size-9 max-sm:size-11" disabled={!human} onClick={() => controls.history("back")}>
              <ArrowLeft aria-hidden />
            </IconButton>
            <IconButton label="Forward" className="size-9 max-sm:size-11" disabled={!human} onClick={() => controls.history("forward")}>
              <ArrowRight aria-hidden />
            </IconButton>
            <IconButton label="Reload" className="size-9 max-sm:size-11" disabled={!human} onClick={() => controls.history("reload")}>
              <RotateCw aria-hidden />
            </IconButton>
            <input
              type="text"
              aria-label="Browser address"
              name="address"
              data-keeps-escape={draft !== null || undefined}
              value={draft ?? tab?.url ?? ""}
              readOnly={!human}
              spellCheck={false}
              autoComplete="off"
              placeholder="Search or enter an address"
              onChange={(event) => setDraft(event.target.value)}
              onKeyDown={(event) => event.key === "Escape" && setDraft(null)}
              className={cn(FIELD, "h-9 min-w-0 flex-1 px-3 text-[13px] max-sm:h-11")}
            />
            <IconButton type="submit" label="Go" className="size-9 max-sm:size-11" disabled={!human} onMouseDown={(event) => event.preventDefault()}><CornerDownLeft aria-hidden /></IconButton>
          </form>
          <div className="flex min-w-0 shrink-0 flex-wrap items-center gap-2 text-[12px] leading-[18px] text-secondary">
            <span className="min-w-0 flex-1" title={human ? "Shift+Tab leaves the page. Closing this panel, or opening Details, hands the browser back to the agent." : undefined}>
              {state.privateActive ? "Private interaction. Browser automation and pictures are paused." : human ? <>{"You're in control."}<span className="sr-only"> Shift+Tab leaves the page.</span></> : state.control === "human" ? "Another device is controlling this browser. You can watch." : `${botName} is using this browser. You can watch.`}
            </span>
            <Pill disabled={state.privateActive} data-focus-return="control" onClick={() => controls.setControl(human ? "bot" : "human")} className="text-[13px] max-sm:min-h-11">
              {human ? "Resume agent" : "Take control"}
            </Pill>
          </div>
          {tab && googleSignInRejected(tab.url) && <p className="text-[12px] leading-[18px] text-secondary">Google may block sign-in in automated browsers.</p>}
          {state.dialog && <PageDialog key={state.dialog.pageId + state.dialog.message} dialog={state.dialog} human={human} controls={controls} />}
        </>
      )}
      {viewport ?? <Viewport view={view} human={human} controls={controls} />}
      {error && (
        <p role="alert" className="flex items-center gap-2 text-[12px] leading-[18px] text-danger">
          <span className="min-w-0 flex-1">{error}</span>
          {!state && (
            <button type="button" onClick={onRetry} className="shrink-0 text-accent hover:underline">
              Try again
            </button>
          )}
        </p>
      )}
    </div>
  );
}

function PageDialog({ dialog, human, controls }: { dialog: { type: string; message: string; defaultValue: string }; human: boolean; controls: BrowserControls }) {
  const [text, setText] = useState(dialog.defaultValue);
  return (
    <div role="alertdialog" data-page-dialog aria-label="The page asks" className="flex flex-col gap-2 rounded-[12px] bg-card p-3 text-[13px] leading-[18px] text-body">
      <p className="break-words">{dialog.message}</p>
      {dialog.type === "prompt" && <input type="text" aria-label="Your answer" value={text} disabled={!human} onChange={(event) => setText(event.target.value)} className="h-8 rounded-[10px] bg-canvas px-3 text-text" />}
      {human ? (
        <div className="flex justify-end gap-2">
          {dialog.type !== "alert" && (
            <button type="button" className={BUTTON} onClick={() => controls.dialog(false)}>
              Cancel
            </button>
          )}
          <button type="button" className={BUTTON} onClick={() => controls.dialog(true, dialog.type === "prompt" ? text : undefined)}>
            OK
          </button>
        </div>
      ) : (
        <p className="text-[12px] text-secondary">Take control to answer.</p>
      )}
    </div>
  );
}

// The stream a frame belongs to, when the daemon names one (a public counter, shown for diagnostics).
function frameStream(frame: { sequence: number }): string | undefined {
  const extra: { sequence: number; streamId?: unknown } = frame;
  return typeof extra.streamId === "string" || typeof extra.streamId === "number" ? String(extra.streamId) : undefined;
}

// True once `on` has been true for `ms`, so a label doesn't flash for a frame that is about to arrive.
function useAfter(on: boolean, ms: number): boolean {
  const [late, setLate] = useState(false);
  useEffect(() => {
    if (!on) return;
    const timer = setTimeout(() => setLate(true), ms);
    return () => {
      clearTimeout(timer);
      setLate(false);
    };
  }, [on, ms]);
  return on && late;
}

// The page as a picture, as large as the pane allows, letterboxed rather than stretched. With the user in control it takes
// the mouse, wheel, keys and paste; otherwise it only shows. A pointer over the letterbox does nothing.
function Viewport({ view, human, controls }: { view: BrowserView; human: boolean; controls: BrowserControls }) {
  const { state } = view;
  const frame = state?.privateActive ? null : view.frame;
  const box = useRef<HTMLDivElement>(null);
  const image = useRef<HTMLImageElement>(null);
  const src = frame ? `data:${frame.mime};base64,${frame.data}` : "";
  const [size, setSize] = useState<Size>({ width: 0, height: 0 });
  const asked = useRef<Size | null>(null);
  const page = state?.activePageId ?? null;
  const busy = "busy" in view && view.busy === true;
  const waiting = useAfter(state !== null && !state.privateActive && (frame === null || busy), WAIT_LABEL_MS);

  // Runs once the picture is committed, so the live stream sends the next only after this one is drawn.
  useEffect(() => {
    if (frame) controls.drawn?.(frame);
  }, [frame, controls]);
  useEffect(() => {
    const element = box.current;
    if (!element) return;
    const observer = new ResizeObserver(([entry]) => setSize({ width: Math.round(entry.contentRect.width), height: Math.round(entry.contentRect.height) }));
    observer.observe(element);
    return () => observer.disconnect();
  }, []);
  // The page is asked for what the viewport shows, once the size settles and only while the user is in control, so a
  // bot's layout isn't disturbed and a drag doesn't flood the daemon.
  useEffect(() => {
    if (!human || !page || size.width <= 0 || size.height <= 0) return;
    const timer = setTimeout(() => {
      const next = viewportSize(size.width, size.height);
      if (!resizeWorthIt(asked.current, next)) return;
      asked.current = next;
      controls.resize(next.width, next.height);
    }, 250);
    return () => clearTimeout(timer);
  }, [human, page, size, controls]);
  useEffect(() => {
    asked.current = null; // another page has its own size
  }, [page]);

  const point = (clientX: number, clientY: number, clamp = false): { x: number; y: number } | null => {
    const rect = box.current?.getBoundingClientRect();
    if (!rect || !frame) return null;
    // A press, move or wheel is mapped through the frame's size, so only once that frame is what's on screen, decoded.
    if (!clamp && !(image.current && imageReady(image.current, frame, src))) return null;
    return (clamp ? framePointClamped : framePoint)(rect, frame, clientX, clientY, frameViewport(frame));
  };
  // React's wheel listener is passive, so this one is added by hand to keep the panel from scrolling instead.
  useEffect(() => {
    const element = box.current;
    if (!element || !human) return;
    const wheel = (event: WheelEvent): void => {
      const at = point(event.clientX, event.clientY);
      if (!at) return;
      event.preventDefault();
      controls.input({ kind: "wheel", ...at, deltaX: wheelPixels(event.deltaX, event.deltaMode), deltaY: wheelPixels(event.deltaY, event.deltaMode) });
    };
    element.addEventListener("wheel", wheel, { passive: false });
    return () => element.removeEventListener("wheel", wheel);
  });

  const mouse = (event: PointerEvent, action: "move" | "down" | "up"): void => {
    // A press, move or wheel over the letterbox is dropped, but a release always reaches the page, at the nearest edge.
    const ready = !!frame && !!image.current && imageReady(image.current, frame, src);
    const at = point(event.clientX, event.clientY, action === "up" && ready);
    const button = action === "move" ? undefined : (mouseButton(event.button) ?? undefined);
    if (at && (action === "move" || button)) controls.input({ kind: "mouse", action, ...at, button });
    // With no picture to point at (it was cleared by a resize or a new page), a release goes by where the press was.
    else if (action === "up") controls.releaseAll();
  };
  const key = (event: KeyboardEvent, action: "down" | "up"): void => {
    if (event.nativeEvent.isComposing) return;
    const route = keyRoute(event);
    if (route === "leave") {
      if (action === "down") controls.releaseAll(); // focus moves on by itself; the page is left with nothing held
      return;
    }
    if (route === "paste") return;
    event.preventDefault();
    controls.input({ kind: "key", action, key: event.key });
  };
  const paste = (event: ClipboardEvent): void => {
    event.preventDefault();
    const text = event.clipboardData.getData("text");
    if (text) controls.input({ kind: "text", text: text.slice(0, 100_000) });
  };

  return (
    <div
      ref={box}
      tabIndex={0}
      role="application"
      aria-label="Browser viewport"
      aria-busy={waiting || undefined}
      data-keeps-escape={human || undefined}
      onKeyDown={human ? (event) => key(event, "down") : undefined}
      onKeyUp={human ? (event) => key(event, "up") : undefined}
      onPaste={human ? paste : undefined}
      onBlur={human ? () => controls.releaseAll() : undefined}
      onPointerDown={
        human
          ? (event) => {
              event.currentTarget.focus();
              event.currentTarget.setPointerCapture(event.pointerId);
              mouse(event, "down");
            }
          : undefined
      }
      onPointerMove={human ? (event) => mouse(event, "move") : undefined}
      onPointerUp={human ? (event) => mouse(event, "up") : undefined}
      onPointerCancel={human ? () => controls.releaseAll() : undefined}
      onContextMenu={human ? (event) => event.preventDefault() : undefined}
      className={cn("group relative min-h-[240px] w-full flex-1 select-none overflow-hidden rounded-[12px] bg-card outline-none focus-visible:ring-2 focus-visible:ring-accent", human ? "cursor-default" : "ring-1 ring-divider")}
    >
      {frame && (
        // eslint-disable-next-line @next/next/no-img-element -- a live JPEG frame from the daemon, not a site asset
        <img
          key={state ? pictureKey(frame, state) : undefined}
          ref={image}
          src={src}
          alt=""
          draggable={false}
          data-viewport-width={frameViewport(frame).width}
          data-viewport-height={frameViewport(frame).height}
          data-frame-sequence={frame.sequence}
          data-frame-stream={frameStream(frame)}
          className={cn("pointer-events-none size-full object-contain", busy && "opacity-60")} />
      )}
      {state && !human && (
        <span className="pointer-events-none absolute bottom-2 left-1/2 -translate-x-1/2 rounded-full bg-popover px-3 py-1 text-[12px] text-text opacity-0 group-hover:opacity-100 group-focus-visible:opacity-100">Take control to interact</span>
      )}
      {waiting && (
        <span role="status" className="pointer-events-none absolute inset-0 flex items-center justify-center gap-2 text-[13px] text-secondary">
          <Loader className="size-4 animate-spin" aria-hidden />
          Waiting for the page…
        </span>
      )}
      {!state && !view.error && <span className="absolute inset-0 flex items-center justify-center text-[13px] text-secondary">Opening the browser…</span>}
    </div>
  );
}
