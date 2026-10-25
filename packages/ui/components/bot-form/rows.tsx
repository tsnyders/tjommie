"use client";

import { useState, type ReactNode } from "react";
import { ChevronDown, ChevronRight, Clock, Folder, Globe, MessageCircle, Shield, type LucideIcon } from "lucide-react";
import type { Autonomy, Bot, Provider } from "@tjommie/protocol";
import { fileName } from "@/lib/attachments";
import { AUTONOMY_LEVELS, autonomyHelp } from "@/lib/bot-form";
import { desktop } from "@/lib/desktop";
import { BotAvatar } from "../bot-avatar";
import { Segmented, Switch } from "../controls";
import { ConfirmDialog } from "../dialog";

// A settings row (WF3): 56 px on the card fill, radius 12, an icon, the title, and the value at the right.
export const ROW_BUTTON = "flex h-14 w-full items-center gap-3 rounded-[12px] px-4 text-left max-sm:h-auto max-sm:min-h-14 max-sm:py-3";

export function RowHeader({ icon: Icon, title, value, chevron }: { icon: LucideIcon; title: string; value?: ReactNode; chevron?: "right" | "down" }) {
  return (
    <>
      <Icon className="size-[18px] shrink-0 text-icon" aria-hidden />
      <span className="min-w-0 flex-1 whitespace-nowrap text-[14px] text-text max-sm:whitespace-normal">{title}</span>
      {value !== undefined && <span className="flex min-w-0 max-w-[60%] items-center gap-1.5 overflow-hidden whitespace-nowrap text-[13px] text-secondary">{value}</span>}
      {chevron === "right" && <ChevronRight className="size-4 shrink-0 text-icon" aria-hidden />}
      {chevron === "down" && <ChevronDown className="size-4 shrink-0 text-icon" aria-hidden />}
    </>
  );
}

// A row that opens in place to show its controls. They start 48 px down, under the title, as Autonomy does in WF3.
export function Row({ icon, title, value, open, onToggle, children }: { icon: LucideIcon; title: string; value?: ReactNode; open: boolean; onToggle: () => void; children: ReactNode }) {
  return (
    <section className="rounded-[12px] bg-card">
      <button type="button" aria-expanded={open} onClick={onToggle} className={ROW_BUTTON}>
        <RowHeader icon={icon} title={title} value={value} chevron={open ? "down" : "right"} />
      </button>
      {open && <div className="-mt-2 min-w-0 px-4 pb-1.5 pl-[46px] max-sm:overflow-x-auto max-sm:pl-4">{children}</div>}
    </section>
  );
}

export function FolderRow({ projectFolder, onChange }: { projectFolder: string | null; onChange: (folder: string | null) => void }) {
  const [open, setOpen] = useState(false);
  async function choose(): Promise<void> {
    const folder = await desktop()?.pickFolder();
    if (folder) onChange(folder);
  }
  return (
    <Row icon={Folder} title="Folder" value={<span className="truncate">{projectFolder ? fileName(projectFolder) : "Its own folder"}</span>} open={open} onToggle={() => setOpen(!open)}>
      <Segmented
        label="Folder"
        value={projectFolder ? "project" : "own"}
        options={[
          { value: "own", label: "Its own folder" },
          { value: "project", label: "A project folder" },
        ]}
        onChange={(value) => (value === "own" ? onChange(null) : void choose())}
      />
      {projectFolder && (
        <p className="mt-3 flex items-center gap-2 text-[12px] leading-[18px] text-secondary">
          <span className="min-w-0 truncate">{projectFolder}</span>
          <button type="button" onClick={() => void choose()} className="shrink-0 text-accent hover:underline">
            Change
          </button>
        </p>
      )}
      <p className="mt-2 text-[12px] leading-[18px] text-secondary">
        {projectFolder ? "It works in the project folder, and keeps its notes and inbox in its own folder." : "It works in its own folder, which Tjommie makes for it."}
      </p>
    </Row>
  );
}

// Full auto asks first (WF11), because it lets the bot do anything the person's Windows account can.
export function AutonomyRow({ autonomy, provider, botName, onChange }: { autonomy: Autonomy; provider: Provider; botName: string; onChange: (autonomy: Autonomy) => void }) {
  const [open, setOpen] = useState(true);
  const [confirming, setConfirming] = useState(false);
  const name = botName.trim() || "this bot";
  return (
    <Row icon={Shield} title="Autonomy" value={open ? undefined : AUTONOMY_LEVELS.find((level) => level.value === autonomy)?.label} open={open} onToggle={() => setOpen(!open)}>
      <Segmented label="Autonomy" value={autonomy} options={AUTONOMY_LEVELS} onChange={(next) => (next === "full" ? setConfirming(true) : onChange(next))} />
      <p className="mt-1.5 text-[12px] leading-[18px] text-secondary">{autonomyHelp(autonomy, provider)}</p>
      <ConfirmDialog
        open={confirming}
        onOpenChange={setConfirming}
        title={`Let ${name} work without asking?`}
        body={`In Full auto, ${name} can run any command and change any file your Windows account can, without checking with you first. Only choose it for bots you trust with that.`}
        confirm="Use Full auto"
        onConfirm={() => {
          onChange("full");
          setConfirming(false);
        }}
      />
    </Row>
  );
}

export function BrowserRow({ browser, onChange }: { browser: boolean; onChange: (browser: boolean) => void }) {
  return (
    <section className="flex h-14 items-center gap-3 rounded-[12px] bg-card px-4">
      <RowHeader icon={Globe} title="Browser" value="Built into Tjommie" />
      <Switch label="Browser" checked={browser} onChange={onChange} />
    </section>
  );
}

// Bots to choose, each with a switch: WF3's Can message, and the bots in a room. A bot's own form leaves the bot out (selfId). `label` is the row's
// title and, before each bot's name, its switch's name; `summary` is the row's value, and `children` the line under the list.
export function CanMessageRow({ label, summary, selfId = null, ids, bots, defaultOpen = false, onChange, children }: {
  label: string;
  summary: string;
  selfId?: string | null;
  ids: string[];
  bots: Bot[];
  defaultOpen?: boolean;
  onChange: (ids: string[]) => void;
  children: ReactNode;
}) {
  const [open, setOpen] = useState(defaultOpen);
  const others = bots.filter((bot) => bot.id !== selfId);
  const chosen = others.filter((bot) => ids.includes(bot.id));
  return (
    <Row
      icon={MessageCircle}
      title={label}
      value={
        <>
          {chosen.slice(0, 3).map((bot) => (
            <BotAvatar key={bot.id} color={bot.color} shape={bot.shape} size={18} className="shrink-0" />
          ))}
          {summary}
        </>
      }
      open={open}
      onToggle={() => setOpen(!open)}
    >
      {others.length === 0 ? (
        <p className="text-[13px] leading-[18px] text-secondary">No other bots yet.</p>
      ) : (
        <ul className="flex flex-col">
          {others.map((bot) => (
            <li key={bot.id} className="flex h-10 items-center gap-3">
              <BotAvatar color={bot.color} shape={bot.shape} size={24} className="shrink-0" />
              <span className="min-w-0 flex-1 truncate text-[14px] text-text">{bot.name}</span>
              <Switch label={`${label} ${bot.name}`} checked={ids.includes(bot.id)} onChange={(on) => onChange(on ? [...ids, bot.id] : ids.filter((id) => id !== bot.id))} />
            </li>
          ))}
        </ul>
      )}
      <p className="mt-2 text-[12px] leading-[18px] text-secondary">{children}</p>
    </Row>
  );
}

export function RoutinesRow({ summary, children }: { summary: string; children: ReactNode }) {
  const [open, setOpen] = useState(false);
  return (
    <Row icon={Clock} title="Routines" value={<span className="truncate">{summary}</span>} open={open} onToggle={() => setOpen(!open)}>
      {children}
    </Row>
  );
}
