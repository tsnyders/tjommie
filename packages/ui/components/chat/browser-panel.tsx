"use client";

import { useEffect, useRef, useState, type ClipboardEvent, type KeyboardEvent, type PointerEvent, type ReactNode } from "react";
import { ArrowLeft, ArrowRight, Loader, Maximize2, Minimize2, Plus, RotateCw, X } from "lucide-react";
import type { Bot, BrowserInput } from "@tjommie/protocol";
import { type BrowserView } from "@/lib/browser-session";
import { desktop, type NativeBrowserApi } from "@/lib/desktop";
import { NativeBrowserSession, nativeView } from "@/lib/native-browser-session";
import { coveredByAppOverlay, slotBounds } from "@/lib/native-overlay";
import { addressToUrl, framePoint, framePointClamped, frameViewport, googleSignInRejected, imageReady, keyRoute, mouseButton, pictureKey, resizeWorthIt, viewportSize, wheelPixels, type Size } from "@/lib/browser-geometry";
import { cn } from "@/lib/utils";
import { IconButton } from "../icon-button";

// The toolbar's calls. (Input, release and resize are for the picture-based fallback viewport; the native page needs none.)
export type BrowserControls = {
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
export function BrowserPanel({ bot, active, expanded, onToggleExpanded }: { bot: Bot; active: boolean; expanded: boolean; onToggleExpanded: () => void }) {
  const api = desktop()?.nativeBrowser;
  if (!api) return <p className="px-4 text-[13px] leading-[18px] text-secondary">The browser needs the Tjommie desktop app.</p>;
  return <NativePane key={bot.id} bot={bot} api={api} active={active} expanded={expanded} onToggleExpanded={onToggleExpanded} />;
}

// Shift+Tab out of the page lands on the control strip's button, where the keyboard would have gone.
const focusControl = (): void => document.querySelector<HTMLElement>('[data-focus-return="control"]')?.focus();

function NativePane({ bot, api, active, expanded, onToggleExpanded }: { bot: Bot; api: NativeBrowserApi; active: boolean; expanded: boolean; onToggleExpanded: () => void }) {
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
      expanded={expanded}
      onToggleExpanded={onToggleExpanded}
      onRetry={() => session.open()}
      viewport={<NativeViewport session={session} active={active} />}
    />
  );
}

// The place the native page sits. The page is a view over the window, so this reports where the slot is, every frame while it
// is shown (the panel moves with a drag, a resize, an expand or a scroll, and none of those is an event of the slot), and
// reports it hidden when the pane isn't shown, the window is, or an app menu, list or dialog is open.
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
      session.setBounds(slotBounds(slot.current?.getBoundingClientRect(), coveredByAppOverlay(document)));
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
  onToggleExpanded,
  onRetry,
  viewport,
}: {
  view: BrowserView;
  botName: string;
  controls: BrowserControls;
  expanded: boolean;
  onToggleExpanded: () => void;
  onRetry: () => void;
  viewport?: ReactNode; // the native page's place; without it, the picture-based viewport
}) {
  const { state, error } = view;
  const human = state?.control === "human";
  const tab = state?.tabs.find((each) => each.id === state.activePageId);
  const [draft, setDraft] = useState<string | null>(null);
  return (
    <div className="flex min-h-0 flex-1 flex-col gap-2 overflow-y-auto px-4 pb-4">
      {state && (
        <>
          <div role="tablist" aria-label="Browser tabs" className="flex items-center gap-1 overflow-x-auto">
            {state.tabs.map((each) => {
              const title = each.title || each.url || "New tab";
              return (
                <div key={each.id} className={cn("flex h-8 min-w-0 max-w-[160px] shrink-0 items-center rounded-[10px]", each.id === state.activePageId ? "bg-selected" : "bg-card")}>
                  <button
                    type="button"
                    role="tab"
                    aria-selected={each.id === state.activePageId}
                    aria-label={`Browser tab ${title}`}
                    title={title}
                    disabled={!human}
                    onClick={() => controls.tab("select", each.id)}
                    className="min-w-0 flex-1 truncate pl-3 pr-1 text-left text-[13px] leading-[18px] text-text disabled:opacity-70"
                  >
                    {title}
                  </button>
                  <button type="button" aria-label={`Close tab ${title}`} disabled={!human} onClick={() => controls.tab("close", each.id)} className="press mr-1 flex size-6 shrink-0 items-center justify-center rounded-[8px] text-icon hover:bg-pill disabled:opacity-50">
                    <X className="size-3.5" aria-hidden />
                  </button>
                </div>
              );
            })}
            <IconButton label="New tab" disabled={!human} onClick={() => controls.tab("new")}>
              <Plus aria-hidden />
            </IconButton>
          </div>
          <form
            className="flex items-center gap-1"
            onSubmit={(event) => {
              event.preventDefault();
              // The form's own value, so a blur that cleared the draft just before this click can't lose the address.
              const typed = new FormData(event.currentTarget).get("address");
              if (typeof typed === "string" && typed.trim()) controls.navigate(addressToUrl(typed));
              setDraft(null);
            }}
          >
            <IconButton label="Back" disabled={!human} onClick={() => controls.history("back")}>
              <ArrowLeft aria-hidden />
            </IconButton>
            <IconButton label="Forward" disabled={!human} onClick={() => controls.history("forward")}>
              <ArrowRight aria-hidden />
            </IconButton>
            <IconButton label="Reload" disabled={!human} onClick={() => controls.history("reload")}>
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
              onChange={(event) => setDraft(event.target.value)}
              onKeyDown={(event) => event.key === "Escape" && setDraft(null)}
              className="h-8 min-w-0 flex-1 rounded-[10px] bg-card px-3 text-[13px] leading-[18px] text-text placeholder:text-caption"
            />
            <button type="submit" disabled={!human} onMouseDown={(event) => event.preventDefault()} className={BUTTON}>
              Go
            </button>
            <IconButton label={expanded ? "Restore browser" : "Expand browser"} data-focus-return="expand" onClick={onToggleExpanded}>
              {expanded ? <Minimize2 aria-hidden /> : <Maximize2 aria-hidden />}
            </IconButton>
          </form>
          <div className="flex items-center justify-between gap-3 text-[12px] leading-[18px] text-secondary">
            <span title={human ? "Closing this panel, or opening Details, hands the browser back to the bot." : undefined}>
              {human ? "You're in control. Shift+Tab leaves the page." : `${botName} is using this browser. You can watch.`}
            </span>
            <button type="button" data-focus-return="control" onClick={() => controls.setControl(human ? "bot" : "human")} className="shrink-0 text-[13px] text-accent hover:underline">
              {human ? "Resume bot" : "Take control"}
            </button>
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
  const { state, frame } = view;
  const box = useRef<HTMLDivElement>(null);
  const image = useRef<HTMLImageElement>(null);
  const src = frame ? `data:${frame.mime};base64,${frame.data}` : "";
  const [size, setSize] = useState<Size>({ width: 0, height: 0 });
  const asked = useRef<Size | null>(null);
  const page = state?.activePageId ?? null;
  const busy = "busy" in view && view.busy === true;
  const waiting = useAfter(state !== null && (frame === null || busy), WAIT_LABEL_MS);

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
