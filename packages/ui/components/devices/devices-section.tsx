"use client";

import { useRef, useState } from "react";
import { Smartphone } from "lucide-react";
import { Dialog as BaseDialog } from "@base-ui/react/dialog";
import type { Device } from "@tjommie/protocol";
import { closeEpoch } from "@/lib/accounts";
import { connectPhone, request } from "@/lib/connection";
import { isAndroidApp } from "@/lib/mobile";
import { errorText } from "@/lib/labels";
import { stamp, timeAgo } from "@/lib/format";
import { FIELD, Label } from "../controls";
import { Dialog } from "../dialog";
import { Pill } from "../pill";
import { PairDialog } from "./pair-dialog";
import { useDevices } from "./use-devices";

type Open = ({ kind: "pair" } | { kind: "rename"; id: string } | { kind: "revoke"; id: string }) & { epoch: number };

// Settings: Devices. The phones and other PCs paired with this one, and a way to pair more. Every paired device has full control of Tjommie, and revoking one
// cuts it off at once. Pairing works on the same Wi-Fi only, and this PC has to be awake: there is no relay and no push yet, and nothing here says otherwise.
export function DevicesSection() {
  const { devices, status, error, reload } = useDevices();
  const [open, setOpen] = useState<Open | null>(null);
  const epochs = useRef(0);
  const show = (dialog: { kind: "pair" } | { kind: "rename"; id: string } | { kind: "revoke"; id: string }): void => setOpen({ ...dialog, epoch: (epochs.current += 1) });
  // A dialog closes only itself: a save that finishes after another dialog opened must not close that one.
  const closer = (epoch: number) => (): void => setOpen((current) => closeEpoch(current, epoch));
  const active = devices?.filter((device) => device.revokedAt === null) ?? [];
  const target = open && "id" in open ? devices?.find((device) => device.id === open.id) : undefined;

  return (
    <>
      <div className="flex items-center justify-between gap-3">
        <h2 className="text-[20px] font-semibold leading-[26px] text-title">Devices</h2>
        <Pill primary onClick={() => show({ kind: "pair" })}>
          Pair a device
        </Pill>
      </div>
      <p className="mt-2 max-w-[700px] text-[13px] leading-[18px] text-secondary">
        Every paired device has full control of Tjommie, including pairing and revoking devices. Devices connect over the same Wi-Fi as this PC, and this PC has to be awake. This PC&apos;s own
        window always works.
      </p>
      {isAndroidApp() && <div className="mt-3"><Pill onClick={() => connectPhone("")}>Switch PC</Pill></div>}
      {status && !status.listening && <p className="mt-3 max-w-[700px] text-[12px] leading-[18px] text-secondary">This PC isn&apos;t accepting devices right now.</p>}
      {error && (
        <p role="alert" className="mt-3 text-[12px] leading-[18px] text-danger">
          {error}
        </p>
      )}
      {!devices ? (
        !error && <p className="mt-8 text-[13px] leading-[18px] text-secondary">Loading devices</p>
      ) : active.length === 0 ? (
        <p className="mt-8 text-[13px] leading-[18px] text-secondary">No devices paired yet.</p>
      ) : (
        <ul aria-label="Paired devices" className="mt-6 flex max-w-[760px] flex-col gap-3">
          {active.map((device) => (
            <DeviceRow key={device.id} device={device} connected={status?.connectedDeviceIds.includes(device.id) ?? false} onRename={() => show({ kind: "rename", id: device.id })} onRevoke={() => show({ kind: "revoke", id: device.id })} />
          ))}
        </ul>
      )}
      {open?.kind === "pair" && <PairDialog onClose={closer(open.epoch)} />}
      {target && open?.kind === "rename" && <RenameDevice device={target} onClose={closer(open.epoch)} onDone={reload} />}
      {target && open?.kind === "revoke" && <RevokeDevice device={target} onClose={closer(open.epoch)} onDone={reload} />}
    </>
  );
}

export function RevokeDevice({ device, onClose, onDone }: { device: Device; onClose: () => void; onDone: () => void }) {
  const pending = useRef(false);
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  async function revoke(): Promise<void> {
    if (pending.current) return;
    pending.current = true; setBusy(true); setProblem(null);
    try {
      await request("devices.revoke", { id: device.id });
      onDone(); onClose();
    } catch (error) {
      setProblem(errorText(error));
    } finally {
      pending.current = false; setBusy(false);
    }
  }
  return (
    <Dialog open onOpenChange={(next) => !next && onClose()} title={`Revoke ${device.name}?`}>
      <BaseDialog.Description className="mt-2 text-[13px] leading-[18px] text-body">It is cut off at once and can&apos;t connect again unless you pair it again. Your other devices aren&apos;t affected.</BaseDialog.Description>
      {problem && <p role="alert" className="mt-3 text-[13px] leading-[18px] text-danger">{problem}</p>}
      <div className="mt-5 flex justify-end gap-2">
        <Pill onClick={onClose}>Cancel</Pill>
        <Pill className="text-danger" disabled={busy} onClick={() => void revoke()}>{busy ? "Revoking…" : "Revoke"}</Pill>
      </div>
    </Dialog>
  );
}

export function DeviceRow({ device, connected, onRename, onRevoke }: { device: Device; connected: boolean; onRename: () => void; onRevoke: () => void }) {
  return (
    <li className="flex flex-wrap items-center gap-3 rounded-[12px] bg-card p-4">
      <Smartphone className="size-5 shrink-0 text-icon" aria-hidden />
      <div className="min-w-0 flex-1">
        <p className="truncate text-[15px] font-semibold leading-[20px] text-title">{device.name}</p>
        <p className="text-[12px] leading-[18px] text-secondary">
          Paired {stamp(device.pairedAt)} · {connected ? "Connected now" : device.lastSeenAt ? `Last seen ${timeAgo(device.lastSeenAt)}` : "Not seen yet"}
        </p>
      </div>
      <div className="flex shrink-0 gap-2">
        <Pill onClick={onRename} aria-label={`Rename ${device.name}`}>
          Rename
        </Pill>
        <Pill className="text-danger" onClick={onRevoke} aria-label={`Revoke ${device.name}`}>
          Revoke
        </Pill>
      </div>
    </li>
  );
}

function RenameDevice({ device, onClose, onDone }: { device: Device; onClose: () => void; onDone: () => void }) {
  return (
    <Dialog open onOpenChange={(next) => !next && onClose()} title="Rename device" width={320} closable>
      <RenameBody device={device} onClose={onClose} onDone={onDone} />
    </Dialog>
  );
}

export function RenameBody({ device, onClose, onDone }: { device: Device; onClose: () => void; onDone: () => void }) {
  const [name, setName] = useState(device.name);
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const changed = name.trim() !== "" && name.trim() !== device.name;
  return (
    <form
      onSubmit={(event) => {
        event.preventDefault();
        if (!changed || busy) return;
        setBusy(true);
        setProblem(null);
        request("devices.rename", { id: device.id, name: name.trim() }).then(
          () => {
            onDone();
            onClose();
          },
          (error: unknown) => {
            setProblem(errorText(error));
            setBusy(false);
          },
        );
      }}
    >
      <div className="mt-4">
        <Label htmlFor="device-name">Name</Label>
        <input id="device-name" value={name} maxLength={80} onChange={(event) => setName(event.target.value)} className={`${FIELD} mt-1.5 h-10`} />
      </div>
      {problem && (
        <p role="alert" className="mt-3 text-[12px] leading-[18px] text-danger">
          {problem}
        </p>
      )}
      <div className="mt-5 flex justify-end gap-2">
        <Pill onClick={onClose}>Cancel</Pill>
        <Pill primary type="submit" disabled={!changed || busy}>
          Save
        </Pill>
      </div>
    </form>
  );
}
