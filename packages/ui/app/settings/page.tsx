"use client";

import Link from "next/link";
import { Suspense, useState, type ReactNode } from "react";
import { useSearchParams } from "next/navigation";
import { Bot, SlidersHorizontal, type LucideIcon } from "lucide-react";
import { Segmented, Stepper } from "@/components/controls";
import { ProviderCards } from "@/components/setup/provider-cards";
import { request } from "@/lib/connection";
import { errorText } from "@/lib/labels";
import { useApp } from "@/lib/store";
import { applyTheme, themeChoice, type ThemeChoice } from "@/lib/theme";
import { cn } from "@/lib/utils";

const THEMES = [
  { value: "dark", label: "Dark" },
  { value: "light", label: "Light" },
  { value: "system", label: "System" },
] as const satisfies readonly { value: ThemeChoice; label: string }[];

// Settings (WF10): General and Bots.
function SettingsScreen() {
  const section = useSearchParams().get("section") === "bots" ? "bots" : "general";
  return (
    <div className="flex min-h-0 flex-1">
      <nav aria-label="Settings" className="flex w-[220px] shrink-0 flex-col border-r border-card-stroke bg-drawer px-3">
        <div className="drag h-11 shrink-0" />
        <h1 className="mb-3 mt-1 px-3 text-[15px] font-semibold leading-[20px] text-title">Settings</h1>
        <div className="flex flex-col gap-1">
          <NavLink href="/settings/" icon={SlidersHorizontal} selected={section === "general"}>
            General
          </NavLink>
          <NavLink href="/settings/?section=bots" icon={Bot} selected={section === "bots"}>
            Bots
          </NavLink>
        </div>
      </nav>
      <div className="flex min-w-0 flex-1 flex-col">
        <div className="drag h-11 shrink-0" />
        <div className="min-h-0 flex-1 overflow-y-auto px-6 pb-10 pt-1">{section === "general" ? <General /> : <BotsSection />}</div>
      </div>
    </div>
  );
}

function NavLink({ href, icon: Icon, selected, children }: { href: string; icon: LucideIcon; selected: boolean; children: ReactNode }) {
  return (
    <Link
      href={href}
      aria-current={selected ? "page" : undefined}
      className={cn("flex h-9 items-center gap-2.5 rounded-[8px] px-3 text-[14px]", selected ? "bg-nav-selected font-medium text-text" : "text-body hover:bg-pill")}
    >
      <Icon className={cn("size-4", selected ? "text-text" : "text-secondary")} aria-hidden />
      {children}
    </Link>
  );
}

function General() {
  // This page renders only in the window, never in the static build, so localStorage is there.
  const [choice, setChoice] = useState<ThemeChoice>(() => themeChoice());
  return (
    <>
      <h2 className="text-[20px] font-semibold leading-[26px] text-title">General</h2>
      <section className="mt-6 flex max-w-[700px] items-center justify-between gap-4 rounded-[12px] bg-card p-4">
        <span className="text-[14px] text-text">Appearance</span>
        <Segmented
          label="Appearance"
          value={choice}
          options={THEMES}
          onChange={(next) => {
            applyTheme(next);
            setChoice(next);
          }}
        />
      </section>
      <h3 className="mt-8 text-[13px] font-semibold leading-[18px] text-label">Claude Code and Codex</h3>
      <div className="mt-3 max-w-[760px]">
        <ProviderCards />
      </div>
    </>
  );
}

function BotsSection() {
  const settings = useApp((state) => state.settings);
  const [problem, setProblem] = useState<string | null>(null);
  async function setChainLimit(chainLimit: number): Promise<void> {
    try {
      await request("settings.set", { chainLimit });
      setProblem(null);
    } catch (error) {
      setProblem(errorText(error));
    }
  }
  return (
    <>
      <h2 className="text-[20px] font-semibold leading-[26px] text-title">Bots</h2>
      <h3 className="mt-8 text-[13px] font-semibold leading-[18px] text-label">Bots asking each other</h3>
      <section className="mt-3 flex max-w-[700px] items-start justify-between gap-6 rounded-[12px] bg-card p-4">
        <div>
          <p className="text-[14px] text-text">Requests between bots per chain</p>
          <p className="mt-1 text-[12px] leading-[18px] text-secondary">
            A chain starts with each message you send and each routine run. When a chain reaches this many bot-to-bot requests, the next request stops and the bot asks you instead.
          </p>
          {problem && (
            <p role="alert" className="mt-1 text-[12px] leading-[18px] text-danger">
              {problem}
            </p>
          )}
        </div>
        {settings && <Stepper label="Requests between bots per chain" value={settings.chainLimit} min={1} max={20} onChange={(next) => void setChainLimit(next)} />}
      </section>
    </>
  );
}

export default function SettingsPage() {
  return (
    <Suspense>
      <SettingsScreen />
    </Suspense>
  );
}
