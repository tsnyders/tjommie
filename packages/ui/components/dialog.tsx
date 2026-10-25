"use client";

import { useEffect, type CSSProperties, type ReactNode, type RefObject } from "react";
import { X } from "lucide-react";
import { Dialog as BaseDialog } from "@base-ui/react/dialog";
import { desktop } from "@/lib/desktop";
import { cn } from "@/lib/utils";
import { Pill } from "./pill";

// Windows draws its caption buttons above the page, out of the scrim's reach, so the desktop app dims them while any
// dialog is open. Open dialogs are counted, and the buttons brighten only once the count stays at zero past the
// current update: a dialog that opens as another closes keeps them dim.
let openDialogs = 0;
let dimmed = false;

function paintCaptionButtons(): void {
  const dim = openDialogs > 0;
  if (dim === dimmed) return;
  dimmed = dim;
  desktop()?.setDialogScrim(dim);
}

// Counts a dialog as open; the returned function counts it closed.
export function dialogOpened(): () => void {
  openDialogs += 1;
  paintCaptionButtons();
  return () => {
    openDialogs -= 1;
    queueMicrotask(paintCaptionButtons);
  };
}

// Dialogs this wide are full screen on a phone (below 640 px): a form needs the room. A narrow one, a question with two buttons, stays a card in the middle.
export const FULL_SCREEN_FROM = 400;
export const fullOnPhone = (width: number): boolean => width >= FULL_SCREEN_FROM;
// The header of a dialog. On a phone's full-screen dialog it is two rows: a top bar of Close at the left and the actions at the right (the actions clear
// of the caption buttons in the desktop window), then the title across the full width below. It is one title element throughout, so its id is single.
// Everywhere else it is the one row it always was.
export function dialogHeader(full: boolean, inApp: boolean): { bar: string; title: string; actions: string; close: string } {
  return {
    bar: cn("flex items-start gap-3", full && "max-sm:flex-wrap max-sm:items-center max-sm:gap-y-1"),
    title: cn("flex-1 text-[17px] font-semibold leading-[22px] text-title", full && "max-sm:order-last max-sm:basis-full max-sm:pb-2 max-sm:pt-1"),
    actions: cn(full && "max-sm:order-2 max-sm:ml-auto max-sm:flex max-sm:min-h-11 max-sm:items-center", full && inApp && "max-sm:mr-[146px]"),
    close: cn("-mr-1 -mt-0.5 flex size-7 items-center justify-center rounded-[8px] text-icon hover:bg-pill hover:text-text", full && "max-sm:order-first max-sm:-ml-2 max-sm:mr-0 max-sm:mt-0 max-sm:size-11"),
  };
}

const FULL = "max-sm:inset-0 max-sm:left-0 max-sm:top-0 max-sm:h-dvh max-sm:max-h-none max-sm:w-auto max-sm:max-w-none max-sm:translate-x-0 max-sm:translate-y-0 max-sm:rounded-none max-sm:border-0 max-sm:px-4 max-sm:pt-0";

type DialogProps = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  width?: number;
  actions?: ReactNode; // beside the title, before the close button
  closable?: boolean;
  initialFocus?: RefObject<HTMLElement | null>;
  className?: string; // the routine editor's wider padding
  children: ReactNode;
};

// Dialogs (WF7, WF11): the card fill with a divider stroke, radius 16, padding 20, over a 60% black scrim.
export function Dialog({ open, onOpenChange, title, width = 320, actions, closable = false, initialFocus, className, children }: DialogProps) {
  useEffect(() => (open ? dialogOpened() : undefined), [open]);
  const full = fullOnPhone(width);
  const header = dialogHeader(full, desktop() !== null);
  return (
    <BaseDialog.Root open={open} onOpenChange={(next) => onOpenChange(next)}>
      <BaseDialog.Portal>
        <BaseDialog.Backdrop className="fixed inset-0 z-40 bg-black/60 transition-opacity duration-200 data-[ending-style]:opacity-0 data-[starting-style]:opacity-0" />
        <BaseDialog.Popup
          initialFocus={initialFocus}
          style={{ "--dialog-width": `${width}px` } as CSSProperties}
          className={cn(
            "mobile-motion-dialog fixed left-1/2 top-1/2 z-50 flex max-h-[calc(100vh-48px)] w-[var(--dialog-width)] max-w-[calc(100vw-48px)] -translate-x-1/2 -translate-y-1/2 flex-col overflow-y-auto rounded-[16px] border border-divider bg-card p-5 transition-[opacity,scale] duration-200 ease-[cubic-bezier(0.23,1,0.32,1)] data-[ending-style]:scale-95 data-[ending-style]:opacity-0 data-[starting-style]:scale-95 data-[starting-style]:opacity-0",
            className,
            full && FULL,
          )}
        >
          <div className={header.bar}>
            <BaseDialog.Title className={header.title}>{title}</BaseDialog.Title>
            {actions && <span className={header.actions}>{actions}</span>}
            {closable && (
              <BaseDialog.Close aria-label="Close" className={header.close}>
                <X className="size-4" aria-hidden />
              </BaseDialog.Close>
            )}
          </div>
          {children}
        </BaseDialog.Popup>
      </BaseDialog.Portal>
    </BaseDialog.Root>
  );
}

// A yes-or-no question (the Full auto warning, Delete bot), with the buttons right-aligned on one line. The body is
// the dialog's description, so it's read out when the dialog opens.
export function ConfirmDialog({ open, onOpenChange, title, body, confirm, danger = false, onConfirm }: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  body: string;
  confirm: ReactNode;
  danger?: boolean;
  onConfirm: () => void;
}) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange} title={title}>
      <BaseDialog.Description className="mt-2 text-[13px] leading-[18px] text-body">{body}</BaseDialog.Description>
      <div className="mt-5 flex justify-end gap-2">
        <Pill onClick={() => onOpenChange(false)}>Cancel</Pill>
        <Pill primary={!danger} className={danger ? "text-danger" : undefined} onClick={onConfirm}>
          {confirm}
        </Pill>
      </div>
    </Dialog>
  );
}
