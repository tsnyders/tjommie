"use client";

import { useEffect, useRef, useState } from "react";
import { Check, Sparkles } from "lucide-react";
import { Popover } from "@base-ui/react/popover";
import type { ModelInfo, Provider } from "@tjommie/protocol";
import { accountHelp, accountsOf, chosenAccount, pickerState, providerName } from "@/lib/accounts";
import { request } from "@/lib/connection";
import { errorText, modelLabel } from "@/lib/labels";
import { useApp } from "@/lib/store";
import { cn } from "@/lib/utils";
import { Segmented } from "../controls";
import { ROW_BUTTON, RowHeader } from "./rows";

const PROVIDERS = [
  { value: "claude", label: "Claude" },
  { value: "codex", label: "Codex" },
] as const satisfies readonly { value: Provider; label: string }[];

export type ModelChoice = { provider: Provider; model: string; accountId: string | null };

// The Account and model row and its picker (WF11): Claude or Codex, that provider's accounts and models, and a caption. `saved` is what an
// existing bot has now, so the line under the row can say a switch starts a fresh session; a new bot has none.
export function ModelRow({ provider, model, accountId, saved, onChange }: { provider: Provider; model: string; accountId: string | null; saved?: { provider: Provider; accountId: string | null }; onChange: (value: ModelChoice) => void }) {
  const accounts = useApp((state) => state.accounts);
  const [open, setOpen] = useState(false);
  const [tab, setTab] = useState<Provider>(provider);
  const [lists, setLists] = useState<Partial<Record<Provider, ModelInfo[]>>>({});
  // Kept per provider, so a list that fails after the person switched tabs doesn't cover the other tab's list.
  const [problems, setProblems] = useState<Partial<Record<Provider, string>>>({});
  const loading = useRef(new Set<Provider>());

  // Codex's list comes from its app-server, so each list is fetched once, when the picker first shows it.
  useEffect(() => {
    if (!open || lists[tab] || problems[tab] || loading.current.has(tab)) return;
    loading.current.add(tab);
    request("models.list", { provider: tab })
      .then(
        (models) => setLists((current) => ({ ...current, [tab]: models })),
        (error: unknown) => setProblems((current) => ({ ...current, [tab]: errorText(error) })),
      )
      .finally(() => loading.current.delete(tab));
  }, [open, tab, lists, problems]);

  const now = new Date();
  const list = accounts ?? [];
  const current = chosenAccount(list, provider, accountId);
  const models = lists[tab];
  const problem = problems[tab];
  const here = accountsOf(list, tab);
  const inTab = chosenAccount(list, tab, provider === tab ? accountId : null);
  const defaultModel = models?.find((info) => info.isDefault)?.id ?? models?.[0]?.id;
  const was = saved ? chosenAccount(list, saved.provider, saved.accountId)?.id : undefined;
  const changed = saved !== undefined && current?.id !== was;
  const help = accounts ? accountHelp(current, now, changed) : null;

  return (
    <>
      <Popover.Root
        open={open}
        onOpenChange={(next) => {
          setOpen(next);
          setProblems({});
          if (next) setTab(provider);
        }}
      >
        <Popover.Trigger className={cn(ROW_BUTTON, "bg-card")}>
          <RowHeader icon={Sparkles} title="Account and model" value={<span className="truncate">{[current?.label, modelLabel({ provider, model })].filter(Boolean).join(" · ")}</span>} chevron="right" />
        </Popover.Trigger>
        <Popover.Portal>
          <Popover.Positioner side="bottom" align="end" sideOffset={6}>
            <Popover.Popup className="mobile-motion-popup max-h-[min(var(--available-height,100dvh),calc(100dvh-32px))] w-[min(320px,calc(100vw-32px))] overflow-y-auto rounded-[16px] border border-divider bg-popover p-2">
              <Segmented
                label="Provider"
                value={tab}
                options={PROVIDERS}
                full
                className="bg-card" // darker than the popover it sits on (WF11)
                onChange={(next) => {
                  setTab(next);
                  setProblems({});
                }}
              />
              <h4 className="px-3 pb-1 pt-3 text-[12px] font-semibold leading-[18px] text-label">Account</h4>
              <div className="flex flex-col gap-1">
                {!accounts ? (
                  <p className="px-3 py-2 text-[13px] leading-[18px] text-secondary">Loading the accounts</p>
                ) : here.length === 0 ? (
                  <p className="px-3 py-2 text-[13px] leading-[18px] text-secondary">No {providerName(tab) === "Codex" ? "Codex" : "Claude"} accounts. Add one in Settings → Accounts.</p>
                ) : (
                  here.map((each) => {
                    const selected = inTab?.id === each.id;
                    const state = pickerState(each, now);
                    return (
                      <button
                        key={each.id}
                        type="button"
                        aria-pressed={selected}
                        // Another provider's account needs one of its models, so it waits for that list.
                        disabled={tab !== provider && defaultModel === undefined}
                        onClick={() => {
                          onChange({ provider: tab, model: tab === provider ? model : (defaultModel ?? model), accountId: each.id });
                          setOpen(false);
                        }}
                        className={cn("flex h-9 items-center justify-between gap-2 rounded-[8px] px-3 text-left text-[14px] text-body hover:bg-highlight disabled:opacity-50", selected && "bg-highlight text-text")}
                      >
                        <span className="min-w-0 truncate">{each.label}</span>
                        <span className="flex shrink-0 items-center gap-2 text-[12px] text-secondary">
                          {state}
                          {selected && <Check className="size-4 text-text" aria-hidden />}
                        </span>
                      </button>
                    );
                  })
                )}
              </div>
              <h4 className="px-3 pb-1 pt-3 text-[12px] font-semibold leading-[18px] text-label">Model</h4>
              <div className="flex flex-col gap-1">
                {problem ? (
                  <p role="alert" className="px-3 py-2 text-[13px] leading-[18px] text-danger">
                    {problem}
                  </p>
                ) : !models ? (
                  <p className="px-3 py-2 text-[13px] leading-[18px] text-secondary">Loading the models</p>
                ) : (
                  models.map((info) => {
                    const selected = provider === tab && model === info.id;
                    return (
                      <button
                        key={info.id}
                        type="button"
                        aria-pressed={selected}
                        onClick={() => {
                          onChange({ provider: tab, model: info.id, accountId: provider === tab ? accountId : (inTab?.id ?? null) });
                          setOpen(false);
                        }}
                        className={cn("flex h-9 items-center justify-between rounded-[8px] px-3 text-left text-[14px] text-body hover:bg-highlight", selected && "bg-highlight text-text")}
                      >
                        {tab === "claude" ? info.label.replace(/^Claude /, "") : info.label}
                        {selected && <Check className="size-4" aria-hidden />}
                      </button>
                    );
                  })
                )}
              </div>
              <p className="px-3 pb-1 pt-2 text-[11px] leading-[16px] text-caption">{tab === "claude" ? "Latest versions are picked automatically." : "Models from Codex's own list."}</p>
            </Popover.Popup>
          </Popover.Positioner>
        </Popover.Portal>
      </Popover.Root>
      {help && <p className="-mt-1 px-1 text-[12px] leading-[18px] text-secondary">{help}</p>}
    </>
  );
}
