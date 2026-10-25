"use client";

import { useEffect, useRef, useState } from "react";
import { Copy } from "lucide-react";
import { QRCodeSVG } from "qrcode.react";
import { request } from "@/lib/connection";
import { errorText } from "@/lib/labels";
import { Dialog } from "../dialog";
import { Pill } from "../pill";
import { countdown } from "./pairing";

type Pairing = { id: string; code: string; uri: string; expires: string };

// Pair a device. Nothing is made until the person presses Show code. The code works once and for ten minutes; closing the dialog, by any route, cancels it. It
// lives only in this dialog's state: not in the address, storage or any log. Mounted only while open, so a new dialog starts clean.
export function PairDialog({ onClose }: { onClose: () => void }) {
  return (
    <Dialog open onOpenChange={(open) => !open && onClose()} title="Pair a device" width={420} closable>
      <PairBody onClose={onClose} />
    </Dialog>
  );
}

export function PairBody({ onClose }: { onClose: () => void }) {
  const [pairing, setPairing] = useState<Pairing | null>(null);
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const [now, setNow] = useState(() => Date.now());
  const open = useRef(true); // false once this dialog is gone: a code that arrives after that is cancelled at once, never shown
  const made = useRef<string | null>(null);

  useEffect(() => {
    open.current = true;
    return () => {
      open.current = false;
      const id = made.current;
      if (id) void request("pairing.cancel", { id }).catch(() => {});
    };
  }, []);
  useEffect(() => {
    if (!pairing) return;
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [pairing]);

  async function show(): Promise<void> {
    setBusy(true);
    setProblem(null);
    setCopied(false);
    try {
      const next = await request("pairing.create", {});
      if (!open.current) {
        void request("pairing.cancel", { id: next.id }).catch(() => {});
        return;
      }
      made.current = next.id;
      setNow(Date.now());
      setPairing(next);
    } catch (error) {
      if (open.current) setProblem(errorText(error));
    } finally {
      if (open.current) setBusy(false);
    }
  }

  async function copy(): Promise<void> {
    if (!pairing) return;
    try {
      await navigator.clipboard.writeText(pairing.code);
      setCopied(true);
    } catch {
      setProblem("Couldn't copy. Select the code and copy it yourself.");
    }
  }

  const left = pairing ? countdown(pairing.expires, now) : null;
  return (
    <div className="mt-3 flex flex-col gap-4 text-[13px] leading-[18px] text-body">
      <p>Open Tjommie on the phone or other PC, and scan this code or paste it there. It works once and lasts ten minutes. It only works on the same Wi-Fi as this PC.</p>
      {pairing && left && !left.expired ? (
        <>
          {/* A QR code must be black on white to scan, whatever the theme: the one place raw colors are right. */}
          <div className="mx-auto w-fit rounded-[12px] bg-white p-3">
            <QRCodeSVG value={pairing.uri} size={192} marginSize={0} bgColor="#ffffff" fgColor="#000000" title="Pairing QR code" />
          </div>
          <p className="text-center text-[12px] text-secondary" role="timer" aria-label="Time left">
            Expires in {left.text}
          </p>
          <div>
            <p className="mb-1.5 text-[12px] text-secondary">Or paste this code</p>
            <p data-testid="pairing-code" className="max-h-24 overflow-y-auto rounded-[10px] border border-input-stroke bg-card p-2 font-mono text-[11px] leading-[16px] text-text [overflow-wrap:anywhere] select-all">
              {pairing.code}
            </p>
          </div>
          <div className="flex justify-end gap-2">
            <Pill onClick={() => void copy()}>
              <Copy className="size-3.5" aria-hidden />
              {copied ? "Copied" : "Copy code"}
            </Pill>
          </div>
        </>
      ) : (
        <div className="flex flex-col gap-3">
          {left?.expired && <p role="status">That code expired. Make a new one.</p>}
          <div className="flex justify-end">
            <Pill primary disabled={busy} onClick={() => void show()}>
              {left?.expired ? "Make a new code" : "Show pairing code"}
            </Pill>
          </div>
        </div>
      )}
      {problem && (
        <p role="alert" className="text-danger">
          {problem}
        </p>
      )}
      <div className="flex justify-end">
        <Pill onClick={onClose}>{pairing ? "Cancel pairing" : "Close"}</Pill>
      </div>
    </div>
  );
}
