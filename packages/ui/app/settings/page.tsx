"use client";

import Link from "next/link";
import { BackLink } from "@/components/back-link";
import { Suspense, useState, type ReactNode } from "react";
import { useSearchParams } from "next/navigation";
import { Bot, ChartColumn, ChevronRight, KeyRound, SlidersHorizontal, Smartphone, type LucideIcon } from "lucide-react";
import { Radio } from "@base-ui/react/radio";
import { RadioGroup } from "@base-ui/react/radio-group";
import { Segmented, Stepper, Switch } from "@/components/controls";
import { AccountsSection } from "@/components/accounts/accounts-section";
import { DevicesSection } from "@/components/devices/devices-section";
import { UsageSection } from "@/components/usage/usage-section";
import { request } from "@/lib/connection";
import { errorText } from "@/lib/labels";
import { useApp } from "@/lib/store";
import { MOBILE_STYLES, mobileStyle, reduceEffects, type MobileStyle } from "@/lib/mobile-style";
import { applyMobileStyle, applyReduceEffects, applyTheme, themeChoice, type ThemeChoice } from "@/lib/theme";
import { usePhone } from "@/lib/use-phone";
import { filtersFrom } from "@/lib/usage";
import { cn } from "@/lib/utils";

const THEMES = [
  { value: "dark", label: "Dark" },
  { value: "light", label: "Light" },
  { value: "system", label: "System" },
] as const satisfies readonly { value: ThemeChoice; label: string }[];

const SECTIONS = ["general", "bots", "accounts", "usage", "devices"] as const;
type Section = (typeof SECTIONS)[number];

// Settings (WF10): General, Bots, Accounts and Usage. Links can name a section, and an account card to point at (never to act on).
function SettingsScreen() {
  const params = useSearchParams();
  const asked = params.get("section");
  const section: Section = SECTIONS.find((each) => each === asked) ?? "general";
  const action = params.get("action");
  return (
    <div className="flex min-h-0 flex-1">
      {/* On a phone Settings is two levels: with no section the list fills the screen, and a section shows "Back to Settings". */}
      <nav aria-label="Settings" data-nav-clear className={cn("flex w-[220px] shrink-0 flex-col border-r border-card-stroke bg-drawer px-3 max-sm:min-h-0 max-sm:w-full max-sm:flex-1 max-sm:overflow-y-auto max-sm:border-r-0 max-sm:bg-canvas max-sm:pb-4", asked !== null && "max-sm:hidden")}>
        <div className="drag h-11 shrink-0" />
        <h1 className="mb-4 mt-2 px-3 text-[15px] font-semibold leading-[20px] text-title">Settings</h1>
        <div className="flex flex-col gap-1">
          <NavLink href="/settings/?section=general" icon={SlidersHorizontal} selected={section === "general"} hint="Appearance and style">
            General
          </NavLink>
          <NavLink href="/settings/?section=bots" icon={Bot} selected={section === "bots"} hint="Requests between bots">
            Bots
          </NavLink>
          <NavLink href="/settings/?section=accounts" icon={KeyRound} selected={section === "accounts"} hint="Sign-ins and limits">
            Accounts
          </NavLink>
          <NavLink href="/settings/?section=usage" icon={ChartColumn} selected={section === "usage"} hint="What the bots used">
            Usage
          </NavLink>
          <NavLink href="/settings/?section=devices" icon={Smartphone} selected={section === "devices"} hint="Pairing with the PC">
            Devices
          </NavLink>
        </div>
      </nav>
      <div className={cn("flex min-w-0 flex-1 flex-col", asked === null && "max-sm:hidden")}>
        <div className="drag h-11 shrink-0 max-sm:hidden" />
        <BackLink href="/settings/" label="Back to Settings">
          Settings
        </BackLink>
        <div data-testid="settings-scroll" data-nav-clear className="min-h-0 flex-1 overflow-y-auto px-6 pb-10 pt-1 max-sm:px-4">
          {section === "general" ? (
            <General />
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

function General() {
  // This page renders only in the window, never in the static build, so localStorage is there.
  const [choice, setChoice] = useState<ThemeChoice>(() => themeChoice());
  const phone = usePhone();
  return (
    <>
      <h2 className="text-[20px] font-semibold leading-[26px] text-title">General</h2>
      <section className="mt-[38px] flex max-w-[700px] items-center justify-between gap-4 rounded-[12px] bg-card p-4">
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
      {phone && <PhoneLook />}
    </>
  );
}

// The phone's own look, kept on this device: Hush, Folio or Aurora, and a switch that trades Aurora's glass and drifting light for solid surfaces.
function PhoneLook() {
  const [style, setStyle] = useState<MobileStyle>(() => mobileStyle());
  const [reduced, setReduced] = useState(() => reduceEffects());
  return (
    <section className="mt-4 rounded-[12px] bg-card p-4">
      <span className="text-[14px] text-text">Style</span>
      <p className="mt-1 text-[12px] leading-[18px] text-secondary">How Tjommie looks on this phone.</p>
      <RadioGroup
        aria-label="Mobile style"
        value={style}
        onValueChange={(next) => {
          const chosen = MOBILE_STYLES.find((each) => each.value === next)?.value ?? "hush";
          applyMobileStyle(chosen);
          setStyle(chosen);
        }}
        className="mt-3 grid grid-cols-3 gap-2"
      >
        {MOBILE_STYLES.map((each) => (
          <Radio.Root
            key={each.value}
            value={each.value}
            aria-label={each.label}
            className="mobile-motion-choice flex h-[72px] cursor-default flex-col items-center justify-center gap-2 rounded-[12px] border border-divider text-[13px] text-secondary data-[checked]:border-text data-[checked]:font-semibold data-[checked]:text-text"
          >
            <span aria-hidden data-swatch={each.value} className="style-swatch" />
            {each.label}
          </Radio.Root>
        ))}
      </RadioGroup>
      <div className="mt-4 flex min-h-11 items-center justify-between gap-4">
        <div>
          <p className="text-[14px] text-text">Reduce effects</p>
          <p className="mt-0.5 text-[12px] leading-[18px] text-secondary">Solid surfaces and still light in Aurora.</p>
        </div>
        <Switch
          label="Reduce effects"
          checked={reduced}
          onChange={(next) => {
            applyReduceEffects(next);
            setReduced(next);
          }}
        />
      </div>
    </section>
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
      <h3 className="mt-9 text-[13px] font-semibold leading-[18px] text-label">Bots asking each other</h3>
      <section className="mt-2.5 flex max-w-[700px] items-start justify-between gap-6 rounded-[12px] bg-card p-4">
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
