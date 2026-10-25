"use client";

import { useState, type ReactNode } from "react";
import Link from "next/link";
import { Menu } from "@base-ui/react/menu";
import { Hourglass, Info, Plug } from "lucide-react";
import type { Bot } from "@tjommie/protocol";
import { bannerFor, money, when, type Banner } from "@/lib/accounts";
import { daemon, request } from "@/lib/connection";
import { errorText } from "@/lib/labels";
import { useApp } from "@/lib/store";
import { cn } from "@/lib/utils";
import { MENU_ITEM, MENU_POPUP } from "../menu";
import { Pill } from "../pill";

const PILL_LINK = "press inline-flex h-8 shrink-0 items-center rounded-full bg-user-bubble px-3.5 text-[14px] font-medium whitespace-nowrap text-user-text";
const accountsLink = (id: string, action: "signin" | "cap" | "retry"): string => `/settings/?section=accounts&account=${encodeURIComponent(id)}&action=${action}`;

// The banner under a bot's chat header (WF11), by what the bot is waiting on: setup (the sandbox, or a provider that has no account to
// name), a sign-in, a spending cap, a limit with accounts it could switch to, or a fallback key already carrying on for it. Waiting colors
// when the person can do something, card colors when it is only news. It never switches accounts on its own, and offers no "always".
export function SetupBanner({ bot }: { bot: Bot }) {
  const accounts = useApp((state) => state.accounts);
  const banner = bannerFor(bot, accounts, new Date());
  return banner && <BannerView bot={bot} banner={banner} />;
}

export function BannerView({ bot, banner }: { bot: Bot; banner: Banner }) {
  switch (banner.kind) {
    case "setup":
      return <ProviderSetupBanner bot={bot} banner={banner} />;
    case "signIn":
      return (
        <Frame icon={<Plug className="size-4 shrink-0 text-accent" aria-hidden />} action={<Link href={accountsLink(banner.account.id, "signin")} className={PILL_LINK}>Sign in</Link>}>
          <p>{banner.account.label} isn&apos;t signed in.</p>
        </Frame>
      );
    case "cap":
      return banner.unknownPrices > 0 ? (
        <Frame icon={<Plug className="size-4 shrink-0 text-accent" aria-hidden />}>
          <p>
            Stopped: {banner.unknownPrices} {banner.unknownPrices === 1 ? "turn has" : "turns have"} no price, so {banner.account.label}&apos;s cap can&apos;t be checked.
          </p>
        </Frame>
      ) : (
        <Frame icon={<Plug className="size-4 shrink-0 text-accent" aria-hidden />} action={<Link href={accountsLink(banner.account.id, "cap")} className={PILL_LINK}>Raise cap</Link>}>
          <p>
            {banner.account.label} reached its {money(banner.account.spending.capUsd ?? 0)} cap.
          </p>
        </Frame>
      );
    case "limited":
      return <LimitedBanner bot={bot} banner={banner} />;
    case "fallback":
      return (
        <Frame quiet icon={<Info className="size-4 shrink-0 text-icon" aria-hidden />}>
          <p>
            {banner.pinned.label} {banner.until ? `is limited until ${when(banner.until, new Date())}` : "is limited, and its reset time isn't known"}. {bot.name} is using {banner.using.label} until then.
          </p>
        </Frame>
      );
  }
}

function Frame({ icon, action, quiet = false, children }: { icon: ReactNode; action?: ReactNode; quiet?: boolean; children: ReactNode }) {
  return (
    <div className={cn("mx-6 mt-2 flex max-w-[600px] items-center gap-3 rounded-[12px] border px-4 py-3", quiet ? "border-card-stroke bg-card" : "border-waiting-stroke bg-waiting-fill")}>
      {icon}
      <div className="min-w-0 flex-1 text-[14px] leading-[21px] text-text">{children}</div>
      {action}
    </div>
  );
}

// A bot that is limited waits for the reset by itself. The person may move it to another account of the same provider that could take
// the work: one button for one, a menu for several, a link to add one for none. A move is one change to that bot's account.
function LimitedBanner({ bot, banner }: { bot: Bot; banner: Extract<Banner, { kind: "limited" }> }) {
  const [problem, setProblem] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  async function moveTo(accountId: string): Promise<void> {
    setBusy(true);
    setProblem(null);
    try {
      await request("bots.update", { id: bot.id, patch: { accountId } });
    } catch (error) {
      setProblem(errorText(error));
    } finally {
      setBusy(false);
    }
  }
  const [only] = banner.alternatives;
  return (
    <div className="mx-6 mt-2 flex max-w-[600px] flex-col gap-2 rounded-[12px] border border-waiting-stroke bg-waiting-fill px-4 py-3">
      {/* On a phone the buttons go under the sentence when they don't fit beside it, and a long account name is cut short in its button. */}
      <div className="flex items-center gap-3 max-sm:flex-wrap">
        <Hourglass className="size-4 shrink-0 text-accent" aria-hidden />
        <p className="min-w-0 flex-1 text-[14px] leading-[21px] text-text [overflow-wrap:anywhere]">
          {banner.account.label} hit its {banner.window}. {bot.name} resumes by itself {banner.until ? `at ${when(banner.until, new Date())}` : "when it resets"}.
        </p>
        {banner.alternatives.length === 1 ? (
          <Pill primary disabled={busy} title={`Switch to ${only.label}`} onClick={() => void moveTo(only.id)} className="min-w-0 max-w-full">
            <span className="truncate">Switch to {only.label}</span>
          </Pill>
        ) : banner.alternatives.length > 1 ? (
          <Menu.Root>
            <Menu.Trigger disabled={busy} className="press inline-flex h-8 shrink-0 items-center rounded-full bg-user-bubble px-3.5 text-[14px] font-medium whitespace-nowrap text-user-text disabled:opacity-50">
              Switch account
            </Menu.Trigger>
            <Menu.Portal>
              <Menu.Positioner side="bottom" align="end" sideOffset={6}>
                <Menu.Popup className={MENU_POPUP}>
                  {banner.alternatives.map((each) => (
                    <Menu.Item key={each.id} className={MENU_ITEM} onClick={() => void moveTo(each.id)}>
                      {each.label}
                    </Menu.Item>
                  ))}
                </Menu.Popup>
              </Menu.Positioner>
            </Menu.Portal>
          </Menu.Root>
        ) : (
          <Link href="/settings/?section=accounts" className={PILL_LINK}>
            Add an account
          </Link>
        )}
        {banner.until === null && (
          <Link href={accountsLink(banner.account.id, "retry")} className="shrink-0 text-[13px] text-accent hover:underline">
            Manage account
          </Link>
        )}
      </div>
      {banner.alternatives.length > 0 && <p className="ml-7 text-[12px] leading-[18px] text-secondary">Switching starts a fresh session. The chat stays.</p>}
      {problem && (
        <p role="alert" className="ml-7 text-[12px] leading-[18px] text-danger">
          {problem}
        </p>
      )}
    </div>
  );
}

// The old setup banner: what a provider needs when no account can be named (its sandbox, or any other reason the daemon gives). When
// setup finishes, the daemon updates the bot and the banner goes away.
function ProviderSetupBanner({ bot, banner }: { bot: Bot; banner: Extract<Banner, { kind: "setup" }> }) {
  const [busy, setBusy] = useState(false);
  const [note, setNote] = useState<string | null>(null);
  const [problem, setProblem] = useState<string | null>(null);

  async function fix(): Promise<void> {
    setBusy(true);
    setProblem(null);
    try {
      if (banner.signIn) {
        await daemon().call("setup.signIn", { provider: bot.provider });
        setNote("Finish signing in in your browser.");
      } else {
        await daemon().call("setup.prepareCodexSandbox", {});
      }
    } catch (error) {
      setProblem(errorText(error));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="mx-6 mt-2 flex max-w-[600px] items-center gap-3 rounded-[12px] border border-waiting-stroke bg-waiting-fill px-4 py-3">
      <Plug className="size-4 shrink-0 text-accent" aria-hidden />
      <div className="min-w-0 flex-1 text-[14px] leading-[21px] text-text">
        <p>{banner.text}</p>
        {note && <p className="text-[12px] leading-[18px] text-secondary">{note}</p>}
        {problem && (
          <p role="alert" className="text-[12px] leading-[18px] text-danger">
            {problem}
          </p>
        )}
      </div>
      {(banner.signIn || banner.sandbox) && (
        <Pill primary disabled={busy} onClick={() => void fix()}>
          {banner.signIn ? "Sign in" : "Set up"}
        </Pill>
      )}
    </div>
  );
}
