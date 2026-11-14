"use client";

import Link from "next/link";
import { BackLink } from "@/components/back-link";
import { Suspense, useState, type ReactNode } from "react";
import { useSearchParams } from "next/navigation";
import { Bell, Bot, ChartColumn, ChevronRight, File, Info, KeyRound, Mic, Palette, SlidersHorizontal, Smartphone, Stethoscope, type LucideIcon } from "lucide-react";
import { Stepper } from "@/components/controls";
import { AccountsSection } from "@/components/accounts/accounts-section";
import { DevicesSection } from "@/components/devices/devices-section";
import { UsageSection } from "@/components/usage/usage-section";
import { AppearanceSection } from "@/components/settings/appearance-section";
import { GeneralSection, NotificationsSection, FilesSection } from "@/components/settings/device-settings";
import { AboutSection, DiagnosticsSection } from "@/components/settings/diagnostics-section";
import { VoiceSection } from "@/components/settings/voice-section";
import { LoginsSection } from "@/components/settings/logins-section";
import { request } from "@/lib/connection";
import { errorText } from "@/lib/labels";
import { useApp } from "@/lib/store";
import { filtersFrom } from "@/lib/usage";
import { cn } from "@/lib/utils";

const SECTIONS = [
  { value: "general", label: "General", icon: SlidersHorizontal, hint: "This device's app" },
  { value: "appearance", label: "Appearance", icon: Palette, hint: "Theme, text and spacing" },
  { value: "notifications", label: "Notifications", icon: Bell, hint: "This device's notifications" },
  { value: "files", label: "Files", icon: File, hint: "Downloads and temporary previews" },
  { value: "voice", label: "Voice", icon: Mic, hint: "Local dictation on this device" },
  { value: "logins", label: "Logins", icon: KeyRound, hint: "Selected host's saved logins" },
  { value: "bots", label: "Agents", icon: Bot, hint: "Host's requests between agents" },
  { value: "accounts", label: "Accounts", icon: KeyRound, hint: "Host's sign-ins and limits" },
  { value: "usage", label: "Usage", icon: ChartColumn, hint: "What the host's agents used" },
  { value: "devices", label: "Devices", icon: Smartphone, hint: "Pairing and host connections" },
  { value: "diagnostics", label: "Diagnostics", icon: Stethoscope, hint: "Health checks and a safe report" },
  { value: "about", label: "About", icon: Info, hint: "Versions, source and licenses" },
] as const;

// Existing section/account/action links still point to the same host controls, never act on an account.
function SettingsScreen() {
  const params = useSearchParams();
  const asked = params.get("section");
  const section = SECTIONS.find((each) => each.value === asked)?.value ?? "general";
  const host = useApp((state) => state.host);
  const action = params.get("action");
  return (
    <div className="settings-surface flex min-h-0 flex-1">
      {/* On a phone Settings is two levels: with no section the list fills the screen, and a section shows "Back to Settings". */}
      <nav aria-label="Settings" data-nav-clear className={cn("flex w-[220px] shrink-0 flex-col overflow-y-auto border-r border-card-stroke bg-drawer px-3 pb-4 max-sm:min-h-0 max-sm:w-full max-sm:flex-1 max-sm:overflow-y-auto max-sm:border-r-0 max-sm:bg-canvas", asked !== null && "max-sm:hidden")}>
        <div className="drag h-11 shrink-0" />
        <h1 className="mb-4 mt-2 px-3 text-[15px] font-semibold leading-[20px] text-title">Settings</h1>
        <div className="flex flex-col gap-1">
          {SECTIONS.map((each) => <NavLink key={each.value} href={`/settings/?section=${each.value}`} icon={each.icon} selected={section === each.value} hint={each.hint}>{each.label}</NavLink>)}
        </div>
      </nav>
      <div className={cn("flex min-w-0 flex-1 flex-col", asked === null && "max-sm:hidden")}>
        <div className="drag h-11 shrink-0 max-sm:hidden" />
        <BackLink href="/settings/" label="Back to Settings">
          Settings
        </BackLink>
        <div key={`${section}:${host ?? "local"}`} data-testid="settings-scroll" data-nav-clear className="min-h-0 flex-1 overflow-y-auto px-6 pb-10 pt-1 max-sm:px-4">
          {section === "general" ? (
            <GeneralSection />
          ) : section === "appearance" ? (
            <AppearanceSection />
          ) : section === "notifications" ? (
            <NotificationsSection />
          ) : section === "files" ? (
            <FilesSection />
          ) : section === "voice" ? (
            <VoiceSection />
          ) : section === "logins" ? (
            <LoginsSection />
          ) : section === "diagnostics" ? (
            <DiagnosticsSection />
          ) : section === "about" ? (
            <AboutSection />
          ) : section === "bots" ? (
            <BotsSection />
          ) : section === "accounts" ? (
            <AccountsSection account={params.get("account")} action={action === "signin" || action === "cap" || action === "retry" ? action : null} />
          ) : section === "devices" ? (
            <DevicesSection />
          ) : (
            <UsageSection filters={filtersFrom(params)} />
          )}
        </div>
      </div>
    </div>
  );
}

// On a phone each section says in a line what's in it.
function NavLink({ href, icon: Icon, selected, hint, children }: { href: string; icon: LucideIcon; selected: boolean; hint: string; children: ReactNode }) {
  return (
    <Link
      href={href}
      aria-current={selected ? "page" : undefined}
      data-slot="settings-link"
      className={cn("flex h-9 items-center gap-2.5 rounded-[8px] px-3 text-[14px] max-sm:h-auto max-sm:min-h-14 max-sm:py-2", selected ? "bg-nav-selected font-medium text-text max-sm:bg-transparent" : "text-body hover:bg-pill")}
    >
      <Icon className={cn("size-4", selected ? "text-text" : "text-secondary")} aria-hidden />
      <span className="flex min-w-0 flex-1 flex-col">
        {children}
        <span className="text-[12px] font-normal leading-[16px] text-secondary sm:hidden">{hint}</span>
      </span>
      <ChevronRight className="size-4 text-icon sm:hidden" aria-hidden />
    </Link>
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
      <h2 className="text-[20px] font-semibold leading-[26px] text-title">Agents</h2>
      <h3 className="mt-9 text-[13px] font-semibold leading-[18px] text-label">Agents asking each other</h3>
      <section className="mt-2.5 flex max-w-[700px] items-start justify-between gap-6 rounded-[12px] bg-card p-4">
        <div>
          <p className="text-[14px] text-text">Requests between agents per chain</p>
          <p className="mt-1 text-[12px] leading-[18px] text-secondary">
            A chain starts with each message you send and each routine run. When a chain reaches this many agent-to-agent requests, the next request stops and the agent asks you instead.
          </p>
          {problem && (
            <p role="alert" className="mt-1 text-[12px] leading-[18px] text-danger">
              {problem}
            </p>
          )}
        </div>
        {settings && <Stepper label="Requests between agents per chain" value={settings.chainLimit} min={1} max={20} onChange={(next) => void setChainLimit(next)} />}
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
