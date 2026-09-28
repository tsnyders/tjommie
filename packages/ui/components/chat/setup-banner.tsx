"use client";

import { useState } from "react";
import { Plug } from "lucide-react";
import { SETUP_CODEX_SANDBOX_REASON, SETUP_SIGN_IN_PREFIX, type Bot } from "@tjommie/protocol";
import { daemon } from "@/lib/connection";
import { errorText } from "@/lib/labels";
import { Pill } from "../pill";

// The Needs setup banner (WF11): waiting-card colors, a plug, what's missing, and the fix when there is one.
// When setup finishes, the daemon updates the bot and the banner goes away.
export function SetupBanner({ bot }: { bot: Bot }) {
  const [busy, setBusy] = useState(false);
  const [note, setNote] = useState<string | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const reason = bot.setupReason ?? "";
  const signIn = reason.startsWith(SETUP_SIGN_IN_PREFIX);
  const sandbox = reason === SETUP_CODEX_SANDBOX_REASON;
  const provider = bot.provider === "claude" ? "Claude" : "Codex";
  const text = signIn ? `${provider} isn't signed in for Tjommie yet.` : sandbox ? "Codex's sandbox isn't set up yet. Windows asks for admin permission once." : reason;

  async function fix(): Promise<void> {
    setBusy(true);
    setProblem(null);
    try {
      if (signIn) {
        await daemon().call("setup.signIn", { provider: bot.provider });
        setNote(bot.provider === "claude" ? "Finish signing in in the window that opened." : "Finish signing in in your browser.");
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
        <p>{text}</p>
        {note && <p className="text-[12px] leading-[18px] text-secondary">{note}</p>}
        {problem && (
          <p role="alert" className="text-[12px] leading-[18px] text-danger">
            {problem}
          </p>
        )}
      </div>
      {(signIn || sandbox) && (
        <Pill primary disabled={busy} onClick={() => void fix()}>
          {signIn ? "Sign in" : "Set up"}
        </Pill>
      )}
    </div>
  );
}
