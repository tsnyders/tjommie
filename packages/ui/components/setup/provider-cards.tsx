"use client";

import { useState, type ReactNode } from "react";
import { CircleCheck, Loader, Sparkles, Terminal } from "lucide-react";
import type { Provider, ProviderSetup } from "@tjommie/protocol";
import { request } from "@/lib/connection";
import { errorText } from "@/lib/labels";
import { cardView, planLabel, PROVIDER_NAME, type CardView } from "@/lib/setup";
import { useApp } from "@/lib/store";
import { cn } from "@/lib/utils";
import { Pill } from "../pill";

const PROVIDERS: Provider[] = ["claude", "codex"];

type Handlers = { onSignIn: () => void; onCancel: () => void; onPrepare: () => void; onCheck: () => void };

// Claude Code and Codex side by side (WF9 first run, and Settings: General). Each card says what's missing and
// offers the one thing that fixes it.
export function ProviderCards() {
  const setup = useApp((state) => state.setup);
  const [signing, setSigning] = useState<{ provider: Provider; since: ProviderSetup | undefined } | null>(null);
  const [pending, setPending] = useState<Provider[]>([]); // setup.signIn calls still waiting for the daemon
  const [preparing, setPreparing] = useState(false);
  const [checking, setChecking] = useState(false);
  const [problem, setProblem] = useState<{ provider: Provider; text: string } | null>(null);
  const setupFor = (provider: Provider): ProviderSetup | undefined => setup?.find((entry) => entry.provider === provider);

  async function run(provider: Provider, action: () => Promise<unknown>): Promise<boolean> {
    setProblem((current) => (current?.provider === provider ? null : current));
    try {
      await action();
      return true;
    } catch (error) {
      setProblem({ provider, text: errorText(error) });
      return false;
    }
  }

  // The daemon can take seconds to answer (Codex starts its app server first). Until it does, Sign in and "Use a
  // different account" stay disabled, so a second click can't open a second sign-in.
  async function signIn(provider: Provider): Promise<void> {
    const since = setupFor(provider);
    setPending((list) => [...list, provider]);
    const ok = await run(provider, () => request("setup.signIn", { provider }));
    setPending((list) => list.filter((entry) => entry !== provider));
    if (ok) setSigning({ provider, since });
  }

  async function prepare(): Promise<void> {
    setPreparing(true);
    await run("codex", () => request("setup.prepareCodexSandbox", {}));
    setPreparing(false);
  }

  // setup.status runs both CLIs' checks, so only this button asks for it.
  async function checkAgain(provider: Provider): Promise<void> {
    setChecking(true);
    await run(provider, () => request("setup.status", {}));
    setChecking(false);
  }

  return (
    <div className="grid grid-cols-[repeat(auto-fit,minmax(300px,1fr))] gap-5">
      {PROVIDERS.map((provider) => {
        const entry = setupFor(provider);
        // Signing in lasts until the daemon reports this provider again: it sends setup.changed when sign-in ends.
        const signingIn = signing?.provider === provider && entry === signing.since;
        const view = cardView(entry, { signingIn, preparing: provider === "codex" && preparing });
        const { body, actions } = content(view, provider === "claude", checking, pending.includes(provider), {
          onSignIn: () => void signIn(provider),
          onCancel: () => setSigning(null),
          onPrepare: () => void prepare(),
          onCheck: () => void checkAgain(provider),
        });
        const Icon = provider === "claude" ? Sparkles : Terminal;
        return (
          <section key={provider} aria-label={PROVIDER_NAME[provider]} className="flex min-h-[200px] flex-col rounded-[14px] border border-card-stroke bg-card p-5">
            <div className="flex items-center gap-4">
              <span className="flex size-9 shrink-0 items-center justify-center rounded-[10px] bg-pill text-icon">
                <Icon className="size-[18px]" aria-hidden />
              </span>
              <div>
                <h2 className="text-[15px] font-semibold leading-[20px] text-title">{PROVIDER_NAME[provider]}</h2>
                <p className="text-[12px] leading-[18px] text-secondary">{entry?.version ? `Version ${entry.version}` : view.kind === "not_installed" ? "Not installed" : "\u00a0"}</p>
              </div>
            </div>
            <div className="mt-7 flex-1 text-[14px] leading-[21px] text-text">{body}</div>
            {/* As tall as a pill even when it holds only a link, so every card's last line sits in the same place. */}
            {actions && <div className="mt-4 flex min-h-8 flex-wrap items-center gap-3">{actions}</div>}
            {problem?.provider === provider && (
              <p role="alert" className="mt-2 text-[12px] leading-[18px] text-danger">
                {problem.text}
              </p>
            )}
          </section>
        );
      })}
    </div>
  );
}

function Line({ icon, className, children }: { icon: ReactNode; className?: string; children: ReactNode }) {
  return (
    <p className={cn("flex items-center gap-2", className)}>
      {icon}
      <span>{children}</span>
    </p>
  );
}

const spinner = <Loader className="size-4 shrink-0 animate-spin text-icon" aria-hidden />;
const helper = "text-[13px] leading-[18px] text-secondary";

function content(view: CardView, claude: boolean, busy: boolean, requesting: boolean, on: Handlers): { body: ReactNode; actions: ReactNode } {
  const checkAgain = (
    <Pill disabled={busy} onClick={on.onCheck}>
      Check again
    </Pill>
  );
  switch (view.kind) {
    case "checking":
      return { body: <Line icon={spinner}>Checking</Line>, actions: null };
    case "not_installed":
      return {
        body: claude ? (
          <p className={helper}>
            Install Claude Code from{" "}
            <a href="https://claude.com/claude-code" target="_blank" rel="noreferrer" className="text-accent hover:underline">
              claude.com/claude-code
            </a>
            , then check again.
          </p>
        ) : (
          <p className={helper}>
            Install Codex with <code className="rounded-[6px] bg-code px-1 py-0.5 text-[12px]">npm install -g @openai/codex</code>, then check again.
          </p>
        ),
        actions: checkAgain,
      };
    case "problem":
      return { body: <p className={helper}>{view.reason}</p>, actions: checkAgain };
    case "signed_out":
      return {
        body: <p className={helper}>{claude ? "Opens Claude's sign-in in a new window." : "Opens OpenAI's sign-in page in your browser."}</p>,
        actions: (
          <Pill primary disabled={requesting} onClick={on.onSignIn}>
            {claude ? "Sign in to Claude" : "Sign in to Codex"}
          </Pill>
        ),
      };
    case "signing_in":
      return {
        body: (
          <>
            <Line icon={spinner}>{claude ? "Finish signing in in the window that opened" : "Finish signing in in your browser"}</Line>
            <p className="ml-6 text-[12px] leading-[18px] text-secondary">Tjommie notices as soon as you&apos;re done.</p>
          </>
        ),
        actions: (
          <>
            <Pill onClick={on.onSignIn}>{claude ? "Open the sign-in window again" : "Open the sign-in page again"}</Pill>
            <button type="button" onClick={on.onCancel} className="text-[14px] text-secondary hover:text-text">
              Cancel
            </button>
          </>
        ),
      };
    case "signed_in": {
      const plan = planLabel(view.plan);
      return {
        body: (
          <>
            <Line icon={<CircleCheck className="size-4 shrink-0 text-working" aria-hidden />}>{view.account ? `Signed in as ${view.account}` : "Signed in"}</Line>
            {plan && <p className="ml-6 text-[12px] leading-[18px] text-secondary">{plan}</p>}
            {view.sandbox === "needed" && (
              <p className={cn(helper, "mt-3")}>Codex&apos;s sandbox isn&apos;t set up yet. Codex bots at Own folder need it, and Windows asks for admin permission once.</p>
            )}
            {view.sandbox === "preparing" && (
              <Line icon={spinner} className="mt-3">
                Setting up Codex&apos;s sandbox. Answer the Windows prompt.
              </Line>
            )}
            {view.sandbox === "unavailable" && <p className={cn(helper, "mt-3")}>Tjommie couldn&apos;t check Codex&apos;s sandbox, so Codex bots at Own folder can&apos;t run yet.</p>}
          </>
        ),
        actions: (
          <>
            {view.sandbox === "needed" && (
              <Pill primary onClick={on.onPrepare}>
                Set up Codex&apos;s sandbox
              </Pill>
            )}
            {view.sandbox === "unavailable" && checkAgain}
            <button type="button" disabled={requesting} onClick={on.onSignIn} className="text-[14px] text-accent enabled:hover:underline disabled:opacity-50">
              Use a different account
            </button>
          </>
        ),
      };
    }
  }
}
