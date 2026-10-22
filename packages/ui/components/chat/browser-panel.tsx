"use client";

import { useEffect, useRef, useState, type ClipboardEvent, type KeyboardEvent, type PointerEvent } from "react";
import { ArrowLeft, ArrowRight, ChevronsRight, Plus, RotateCw, X } from "lucide-react";
import type { Bot } from "@tjommie/protocol";
import { addressToUrl, BrowserSession, keyRoute, mouseButton, pagePoint, viewportSize, wheelPixels, type BrowserView } from "@/lib/browser-session";
import { request } from "@/lib/connection";
import { cn } from "@/lib/utils";
import { IconButton } from "../icon-button";

export type BrowserControls = Pick<BrowserSession, "input" | "releaseAll" | "setControl" | "navigate" | "history" | "resize" | "dialog" | "tab">;

const BUTTON = "press h-8 shrink-0 rounded-[10px] bg-pill px-3 text-[13px] leading-[18px] text-text hover:bg-selected disabled:opacity-50";

// The bot's own browser, shown in the right panel (WF1 Phase 1). Hiding the panel stops the pulls and lets go of any held
// key; the browser itself stays open for the bot.
export function BrowserPanel({ bot, onClose }: { bot: Bot; onClose: () => void }) {
  const [view, setView] = useState<BrowserView>({ state: null, frame: null, error: null });
  const session = useRef<BrowserSession | null>(null);
  useEffect(() => {
    const next = new BrowserSession(bot.id, request, setView);
    session.current = next;
    next.open();
    const visibility = (): void => next.setVisible(document.visibilityState === "visible");
    document.addEventListener("visibilitychange", visibility);
    return () => {
      document.removeEventListener("visibilitychange", visibility);
      void next.close();
      session.current = null;
    };
  }, [bot.id]);
  // The controls reach whichever session is live, and do nothing once it's closed.
  const [controls] = useState<BrowserControls>(() => ({
    input: (input) => session.current?.input(input),
    releaseAll: () => session.current?.releaseAll(),
    setControl: (control) => session.current?.setControl(control),
    navigate: (url) => session.current?.navigate(url),
    history: (action) => session.current?.history(action),
    resize: (width, height) => session.current?.resize(width, height),
    dialog: (accept, text) => session.current?.dialog(accept, text),
    tab: (action, pageId) => session.current?.tab(action, pageId),
  }));
  const retry = (): void => session.current?.open();
  return <BrowserChrome view={view} botName={bot.name} controls={controls} onClose={onClose} onRetry={retry} />;
}

export function BrowserChrome({ view, botName, controls, onClose, onRetry }: { view: BrowserView; botName: string; controls: BrowserControls; onClose: () => void; onRetry: () => void }) {
  const { state, frame, error } = view;
  const human = state?.control === "human";
  const tab = state?.tabs.find((each) => each.id === state.activePageId);
  const [draft, setDraft] = useState<string | null>(null);
  return (
    <aside aria-label={`${botName}'s browser`} className="flex w-[560px] max-w-[60vw] shrink-0 flex-col border-l border-divider bg-canvas">
      <div className="drag flex h-11 shrink-0 items-center justify-between px-4">
        <h2 className="text-[13px] font-semibold leading-[18px] text-label">Browser</h2>
        <IconButton label="Close browser panel" onClick={onClose}>
          <ChevronsRight aria-hidden />
        </IconButton>
      </div>
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
            </form>
            <div className="flex items-center justify-between gap-3 text-[12px] leading-[18px] text-secondary">
              <span>{human ? "You're in control. Closing this panel hands the browser back to the bot. Shift+Tab leaves the page." : `${botName} is using this browser. You can watch.`}</span>
              <button type="button" onClick={() => controls.setControl(human ? "bot" : "human")} className="shrink-0 text-[13px] text-accent hover:underline">
                {human ? "Resume bot" : "Take control"}
              </button>
            </div>
            {state.dialog && <PageDialog key={state.dialog.pageId + state.dialog.message} dialog={state.dialog} human={human} controls={controls} />}
          </>
        )}
        <Viewport view={view} human={human} controls={controls} />
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
        {frame === null && !error && <p className="text-[12px] leading-[18px] text-secondary">Opening the browser…</p>}
      </div>
    </aside>
  );
}

function PageDialog({ dialog, human, controls }: { dialog: { type: string; message: string; defaultValue: string }; human: boolean; controls: BrowserControls }) {
  const [text, setText] = useState(dialog.defaultValue);
  return (
    <div role="alertdialog" aria-label="The page asks" className="flex flex-col gap-2 rounded-[12px] bg-card p-3 text-[13px] leading-[18px] text-body">
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

// The page as a picture. With the user in control it takes the mouse, wheel, keys and paste; otherwise it only shows.
function Viewport({ view, human, controls }: { view: BrowserView; human: boolean; controls: BrowserControls }) {
  const { state, frame } = view;
  const box = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(0);
  const page = state?.activePageId ?? null;

  // The page is asked for the shown width, only while the user is in control, so a bot's layout isn't disturbed.
  useEffect(() => {
    const element = box.current;
    if (!element) return;
    const observer = new ResizeObserver(([entry]) => setWidth(Math.round(entry.contentRect.width / 8) * 8));
    observer.observe(element);
    return () => observer.disconnect();
  }, []);
  useEffect(() => {
    if (!human || !page || width <= 0) return;
    const timer = setTimeout(() => {
      const size = viewportSize(width);
      controls.resize(size.width, size.height);
    }, 250);
    return () => clearTimeout(timer);
  }, [human, page, width, controls]);

  const point = (clientX: number, clientY: number): { x: number; y: number } | null => {
    const rect = box.current?.getBoundingClientRect();
    return rect && frame ? pagePoint(rect, frame, clientX, clientY) : null;
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
    const at = point(event.clientX, event.clientY);
    const button = action === "move" ? undefined : (mouseButton(event.button) ?? undefined);
    if (at && (action === "move" || button)) controls.input({ kind: "mouse", action, ...at, button });
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
      className={cn("relative aspect-[4/3] w-full select-none overflow-hidden rounded-[12px] bg-card outline-none focus-visible:ring-2 focus-visible:ring-accent", human ? "cursor-default" : "ring-1 ring-divider")}
    >
      {frame && (
        // eslint-disable-next-line @next/next/no-img-element -- a live JPEG frame from the daemon, not a site asset
        <img src={`data:${frame.mime};base64,${frame.data}`} alt="" draggable={false} className="pointer-events-none size-full object-fill" />
      )}
    </div>
  );
}
