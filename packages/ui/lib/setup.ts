import type { Provider, ProviderSetup } from "@tjommie/protocol";

export const PROVIDER_NAME: Record<Provider, string> = { claude: "Claude Code", codex: "Codex" };

export type SandboxView = "ready" | "needed" | "preparing" | "unavailable";
export type CardView =
  | { kind: "checking" }
  | { kind: "not_installed" }
  | { kind: "signing_in" }
  | { kind: "signed_in"; account: string | null; plan: string | null; sandbox: SandboxView | null }
  | { kind: "problem"; reason: string }
  | { kind: "signed_out" };

// What a provider's setup card shows (WF9). `signingIn` and `preparing` belong to this window: the person clicked
// Sign in or Set up, and the daemon hasn't reported the outcome yet.
export function cardView(setup: ProviderSetup | undefined, local: { signingIn: boolean; preparing: boolean }): CardView {
  if (!setup) return { kind: "checking" };
  if (!setup.installed) return { kind: "not_installed" };
  if (local.signingIn) return { kind: "signing_in" };
  if (setup.signedIn) {
    let sandbox: SandboxView | null = null;
    if (setup.provider === "codex") {
      if (setup.sandbox === "ready") sandbox = "ready";
      else if (local.preparing) sandbox = "preparing";
      else sandbox = setup.sandbox === "unavailable" ? "unavailable" : "needed";
    }
    return { kind: "signed_in", account: setup.account, plan: setup.plan, sandbox };
  }
  if (setup.error) return { kind: "problem", reason: setup.error };
  return { kind: "signed_out" };
}

// Both providers' entries when the setup check itself fails, so each card shows the reason and Check again
// instead of "Checking" forever.
export function failedSetup(reason: string): ProviderSetup[] {
  const failed = { installed: true, version: null, signedIn: false, account: null, plan: null, error: reason };
  return [
    { provider: "claude", ...failed },
    { provider: "codex", ...failed, sandbox: "unavailable" },
  ];
}

// "Max plan", from the CLI's plan name.
export const planLabel = (plan: string | null): string | null => (plan ? `${plan.slice(0, 1).toUpperCase()}${plan.slice(1)} plan` : null);
