"use client";

import { useState } from "react";
import { Menu } from "@base-ui/react/menu";
import { Check, Monitor, Plus } from "lucide-react";
import { connectLocal, connectRemote, forgetRemote } from "@/lib/connection";
import { pairRemote, remoteHosts } from "@/lib/remote-hosts";
import type { Connection } from "@/lib/state";
import { useApp } from "@/lib/store";
import { cn } from "@/lib/utils";
import { Dialog } from "./dialog";
import { MENU_ITEM, MENU_POPUP } from "./menu";
import { MobileConnect } from "./mobile-connect";

export const THIS_PC = "this-pc"; // the desktop's own Tjommie in the list; saved PCs are their ids
type HostStatus = "Connected" | "Connecting" | "Offline" | "Not connected";
const DOT: Record<HostStatus, string> = { Connected: "bg-working", Connecting: "bg-accent", Offline: "bg-danger", "Not connected": "bg-caption" };

// The switcher's list: This PC on the desktop, then the saved PCs. Only the PC in use has a known state; the others aren't connected.
export function hostRows(hosts: { id: string; name: string }[], current: string | null, connection: Connection, desktop: boolean): { value: string; label: string; status: HostStatus | null }[] {
  const status = (id: string): HostStatus => {
    if (id !== current) return "Not connected";
    if (connection.status === "ready") return "Connected";
    if (connection.status === "connecting" || connection.status === "reconnecting") return "Connecting";
    return connection.status === "offline" ? "Offline" : "Not connected";
  };
  return [...(desktop ? [{ value: THIS_PC, label: "This PC", status: null }] : []), ...hosts.map((host) => ({ value: host.id, label: host.name, status: status(host.id) }))];
}

// This PC goes back to the desktop's own Tjommie, and a saved PC connects to it. The PC in use is left alone, so its link isn't dropped.
export function chooseHost(value: string, current: string | null): void {
  if (value === (current ?? THIS_PC)) return;
  if (value === THIS_PC) connectLocal();
  else connectRemote(value);
}

// Which PC this window works on. On the desktop it is the rail's avatar, showing the PC's name while on another one; on a phone it is the Switch PC
// pill. The list ends with "Connect to another computer…": the desktop pastes that PC's code in a dialog, a phone goes to its connect screen.
export function HostSwitcher({ phone = false, initial = "" }: { phone?: boolean; initial?: string }) {
  const current = useApp((state) => state.host);
  const connection = useApp((state) => state.connection);
  const [open, setOpen] = useState(false); // held here so each opening reads the saved PCs again
  const [connecting, setConnecting] = useState(false);
  const hosts = remoteHosts();
  const label = current === null ? "This PC" : (hosts.find((host) => host.id === current)?.name ?? "Another PC");
  return (
    <>
      <Menu.Root open={open} onOpenChange={setOpen}>
        {phone ? (
          <Menu.Trigger className="press inline-flex h-8 shrink-0 items-center gap-1.5 rounded-full bg-pill px-3.5 text-[14px] font-medium text-text">Switch PC</Menu.Trigger>
        ) : (
          <Menu.Trigger aria-label={`Switch PC, now on ${label}`} title={label} className="-mb-1 mt-2 flex max-w-[80px] flex-col items-center gap-1 rounded-[12px] p-1 hover:bg-pill data-[popup-open]:bg-selected">
            {current === null ? (
              <span aria-hidden className="flex size-8 items-center justify-center rounded-full bg-user-avatar text-[13px] font-semibold text-white">{initial}</span>
            ) : (
              <>
                <span aria-hidden className="flex size-8 items-center justify-center rounded-full bg-pill">
                  <Monitor className="size-4 text-icon" />
                </span>
                <span className="w-full truncate text-[11px] leading-[14px] text-secondary">{label}</span>
              </>
            )}
          </Menu.Trigger>
        )}
        <Menu.Portal>
          <Menu.Positioner side={phone ? "bottom" : "right"} align={phone ? "start" : "end"} sideOffset={8}>
            <Menu.Popup className={MENU_POPUP}>
              <Menu.RadioGroup value={current ?? THIS_PC} onValueChange={(value: unknown) => { if (typeof value === "string") chooseHost(value, current); }}>
                {hostRows(hosts, current, connection, !phone).map((row) => (
                  <Menu.RadioItem key={row.value} value={row.value} closeOnClick className={MENU_ITEM}>
                    {row.status === null ? <Monitor aria-hidden /> : <span aria-hidden title={row.status} className={cn("mx-[3px] size-2.5 shrink-0 rounded-full", DOT[row.status])} />}
                    <span className="min-w-0 flex-1 truncate">{row.label}</span>
                    {row.status && <span className="sr-only">, {row.status}</span>}
                    <Menu.RadioItemIndicator>
                      <Check aria-hidden />
                    </Menu.RadioItemIndicator>
                  </Menu.RadioItem>
                ))}
              </Menu.RadioGroup>
              <Menu.Separator className="mx-1.5 my-1 h-px bg-divider" />
              <Menu.Item className={MENU_ITEM} onClick={() => (phone ? connectRemote("") : setConnecting(true))}>
                <Plus aria-hidden />
                Connect to another computer…
              </Menu.Item>
            </Menu.Popup>
          </Menu.Positioner>
        </Menu.Portal>
      </Menu.Root>
      {connecting && <ConnectDialog onClose={() => setConnecting(false)} />}
    </>
  );
}

// "Connect to another computer…" (and Pair again once a PC removed this desktop): the shared connect screen, without a scanner. Connecting to a PC,
// saved or just paired, switches this window to it.
export function ConnectDialog({ onClose }: { onClose: () => void }) {
  const connect = (id?: string): void => {
    onClose();
    if (id) connectRemote(id);
  };
  return (
    <Dialog open onOpenChange={(next) => !next && onClose()} title="Connect to another computer" width={440} closable>
      <MobileConnect status="outside" device="pc" dialog api={{ hosts: remoteHosts, pair: pairRemote, connect, forget: forgetRemote }} />
    </Dialog>
  );
}
