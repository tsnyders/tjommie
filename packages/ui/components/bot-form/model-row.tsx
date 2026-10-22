"use client";

import { useEffect, useRef, useState } from "react";
import { Check, Sparkles } from "lucide-react";
import { Popover } from "@base-ui/react/popover";
import type { ModelInfo, Provider } from "@tjommie/protocol";
import { request } from "@/lib/connection";
import { errorText, modelLabel } from "@/lib/labels";
import { cn } from "@/lib/utils";
import { Segmented } from "../controls";
import { ROW_BUTTON, RowHeader } from "./rows";

const PROVIDERS = [
  { value: "claude", label: "Claude" },
  { value: "codex", label: "Codex" },
] as const satisfies readonly { value: Provider; label: string }[];

// The Model row and its picker (WF11): Claude or Codex, that provider's models, and a caption.
export function ModelRow({ provider, model, onChange }: { provider: Provider; model: string; onChange: (value: { provider: Provider; model: string }) => void }) {
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

  const models = lists[tab];
  const problem = problems[tab];
  return (
    <Popover.Root
      open={open}
      onOpenChange={(next) => {
        setOpen(next);
        setProblems({});
        if (next) setTab(provider);
      }}
    >
      <Popover.Trigger className={cn(ROW_BUTTON, "bg-card")}>
        <RowHeader icon={Sparkles} title="Model" value={<span className="truncate">{modelLabel({ provider, model })}</span>} chevron="right" />
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Positioner side="bottom" align="end" sideOffset={6}>
          <Popover.Popup className="w-[320px] rounded-[16px] border border-divider bg-popover p-2">
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
            <div className="mt-1 flex flex-col gap-1">
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
                        onChange({ provider: tab, model: info.id });
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
  );
}
