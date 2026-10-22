"use client";

import Link from "next/link";
import { useRouter } from "next/navigation";
import { useEffect } from "react";
import { ProviderCards } from "@/components/setup/provider-cards";
import { useApp } from "@/lib/store";

// Opens the first bot's chat. With no bots yet, it's first-run setup (WF9).
export default function Home() {
  const router = useRouter();
  const first = useApp((state) => state.bots[0]?.id);
  useEffect(() => {
    if (first) router.replace(`/chat/?bot=${encodeURIComponent(first)}`);
  }, [first, router]);
  if (first) return null;
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="drag h-11 shrink-0" />
      <div className="min-h-0 flex-1 overflow-y-auto">
        <div className="mx-auto w-full max-w-[808px] px-6 pb-12 pt-[72px]">
          <h1 className="text-[20px] font-semibold leading-[26px] text-title">Set up Tjommie</h1>
          <p className="mt-2 text-[14px] leading-[21px] text-secondary">
            Tjommie runs your bots with Claude Code and Codex on this PC. Sign in to each one once. Tjommie keeps its own sign-in, separate from how you use these tools yourself, and never sees your password.
          </p>
          <div className="mt-8">
            <ProviderCards />
          </div>
          <Link href="/bots/new/" className="press mt-10 inline-flex h-10 items-center rounded-full bg-user-bubble px-5 text-[14px] font-semibold text-user-text">
            Create your first bot
          </Link>
          <p className="mt-3 text-[12px] leading-[18px] text-secondary">You can sign in to the other one later in Settings.</p>
        </div>
      </div>
    </div>
  );
}
