"use client";

import { useState, type ReactNode } from "react";
import Link from "next/link";
import { Eye, EyeOff } from "lucide-react";
import type { AccountInfo, Bot, Provider } from "@tjommie/protocol";
import { botsOn, keyReady, money, parseCap, providerName, takeKey } from "@/lib/accounts";
import { errorText } from "@/lib/labels";
import { Dialog } from "../dialog";
import { FIELD, Label, Switch } from "../controls";
import { IconButton } from "../icon-button";
import { Pill } from "../pill";

// Account dialogs. Each is mounted only while it is open, and keeps what is typed in its own body, so closing it, by any route,
// throws that away. The API key in particular lives only in the key field's state: never in the store, the address or storage,
// never logged, and never shown again once it has been sent.

// A dialog's form: the fields, the error line, and the buttons. Enter or the main button runs it; a failure shows its message above the
// buttons and keeps what was typed.
function Form({ children, submit, canSubmit, onCancel, run, danger = false }: { children: ReactNode; submit: string; canSubmit: boolean; onCancel: () => void; run: () => Promise<void>; danger?: boolean }) {
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  return (
    <form
      onSubmit={(event) => {
        event.preventDefault();
        if (!canSubmit || busy) return;
        setBusy(true);
        setProblem(null);
        run()
          .catch((error: unknown) => setProblem(errorText(error)))
          .finally(() => setBusy(false));
      }}
    >
      {children}
      {problem && (
        <p role="alert" className="mt-3 text-[12px] leading-[18px] text-danger">
          {problem}
        </p>
      )}
      <div className="mt-5 flex justify-end gap-2 max-sm:sticky max-sm:bottom-0 max-sm:-mx-4 max-sm:border-t max-sm:border-divider max-sm:bg-card max-sm:px-4 max-sm:py-3">
        <Pill onClick={onCancel}>Cancel</Pill>
        <Pill primary={!danger} className={danger ? "text-danger" : undefined} type="submit" disabled={!canSubmit || busy}>
          {submit}
        </Pill>
      </div>
    </form>
  );
}

function Field({ id, label, children, help }: { id: string; label: string; children: ReactNode; help?: ReactNode }) {
  return (
    <div className="mt-4">
      <Label htmlFor={id}>{label}</Label>
      <div className="mt-1.5">{children}</div>
      {help && <p className="mt-1.5 text-[12px] leading-[18px] text-secondary">{help}</p>}
    </div>
  );
}

function NameField({ id = "account-name", value, onChange, placeholder }: { id?: string; value: string; onChange: (value: string) => void; placeholder: string }) {
  return (
    <Field id={id} label="Name">
      <input id={id} value={value} maxLength={60} placeholder={placeholder} onChange={(event) => onChange(event.target.value)} className={`${FIELD} h-10`} />
    </Field>
  );
}

// The key is a password field with a Show toggle, and the box is told not to remember or correct it.
function KeyField({ provider, value, onChange }: { provider: Provider; value: string; onChange: (value: string) => void }) {
  const [shown, setShown] = useState(false);
  return (
    <Field id="account-key" label="API key" help={`Stored in Windows Credential Manager and given only to the ${providerName(provider)} process.`}>
      <div className="relative">
        <input
          id="account-key"
          type={shown ? "text" : "password"}
          value={value}
          onChange={(event) => onChange(event.target.value)}
          autoComplete="off"
          spellCheck={false}
          autoCapitalize="off"
          autoCorrect="off"
          className={`${FIELD} h-10 pr-11`}
        />
        <IconButton label={shown ? "Hide key" : "Show key"} active={shown} onClick={() => setShown(!shown)} className="absolute right-1 top-1">
          {shown ? <EyeOff aria-hidden /> : <Eye aria-hidden />}
        </IconButton>
      </div>
    </Field>
  );
}

function CapField({ value, onChange, below }: { value: string; onChange: (value: string) => void; below?: string | null }) {
  return (
    <Field id="account-cap" label="Spending cap" help={below ?? "Bots stop using the key once its total spend reaches this. It's an estimate from the prices Tjommie knows, not an invoice. Your provider bill may be higher."}>
      <div className="relative">
        <span aria-hidden className="pointer-events-none absolute left-4 top-0 flex h-10 items-center text-[14px] text-secondary">
          $
        </span>
        <input id="account-cap" type="number" inputMode="decimal" min={0.01} step={0.01} required value={value} onChange={(event) => onChange(event.target.value)} className={`${FIELD} h-10 pl-8`} />
      </div>
    </Field>
  );
}

const close = (onClose: () => void) => (open: boolean) => !open && onClose();

export function AddSubscriptionDialog({ provider, onClose, onSubmit }: { provider: Provider; onClose: () => void; onSubmit: (label: string) => Promise<void> }) {
  return (
    <Dialog open onOpenChange={close(onClose)} title={`Add ${providerName(provider)} subscription`} width={420} closable>
      <SubscriptionBody provider={provider} onClose={onClose} onSubmit={onSubmit} />
    </Dialog>
  );
}

export function SubscriptionBody({ provider, onClose, onSubmit }: { provider: Provider; onClose: () => void; onSubmit: (label: string) => Promise<void> }) {
  const [label, setLabel] = useState("");
  return (
    <Form
        submit="Add and sign in"
        canSubmit={label.trim().length > 0}
        onCancel={onClose}
        run={async () => {
          await onSubmit(label.trim());
          onClose();
        }}
    >
      <NameField value={label} onChange={setLabel} placeholder={`${provider === "claude" ? "Claude" : "Codex"} (Personal)`} />
      <p className="mt-3 text-[12px] leading-[18px] text-secondary">Signing in opens the provider&apos;s own page in your browser. Tjommie never sees your password.</p>
    </Form>
  );
}

export type NewKey = { label: string; apiKey: string; spendingCapUsd: number; isFallback: boolean };

export function AddKeyDialog({ provider, onClose, onSubmit }: { provider: Provider; onClose: () => void; onSubmit: (key: NewKey) => Promise<void> }) {
  return (
    <Dialog open onOpenChange={close(onClose)} title={`Add ${providerName(provider)} API key`} width={420} closable>
      <KeyBody provider={provider} onClose={onClose} onSubmit={onSubmit} />
    </Dialog>
  );
}

export function KeyBody({ provider, onClose, onSubmit }: { provider: Provider; onClose: () => void; onSubmit: (key: NewKey) => Promise<void> }) {
  const [label, setLabel] = useState("");
  const [apiKey, setApiKey] = useState("");
  const [cap, setCap] = useState("");
  const [fallback, setFallback] = useState(false);
  const amount = parseCap(cap);
  return (
    <Form
        submit="Add key"
        canSubmit={label.trim().length > 0 && keyReady(apiKey) && amount !== null}
        onCancel={onClose}
        run={async () => {
          if (amount === null) return;
          const secret = takeKey(apiKey, () => setApiKey(""));
          await onSubmit({ label: label.trim(), apiKey: secret, spendingCapUsd: amount, isFallback: fallback });
          onClose();
        }}
    >
      <NameField value={label} onChange={setLabel} placeholder={`${provider === "claude" ? "Claude" : "Codex"} (API key)`} />
      <KeyField provider={provider} value={apiKey} onChange={setApiKey} />
      <CapField value={cap} onChange={setCap} />
      <div className="mt-4 flex items-start justify-between gap-4">
        <div>
          <p className="text-[14px] leading-[21px] text-text">Fallback for {provider === "claude" ? "Claude" : "Codex"}</p>
          <p className="text-[12px] leading-[18px] text-secondary">When a {provider === "claude" ? "Claude" : "Codex"} subscription account hits its limit, bots carry on with this key until it resets.</p>
        </div>
        <Switch label={`Fallback for ${provider === "claude" ? "Claude" : "Codex"}`} checked={fallback} onChange={setFallback} />
      </div>
    </Form>
  );
}

// The key goes alone: replacing it is never combined with changing anything else about the account.
export function ReplaceKeyDialog({ account, onClose, onSubmit }: { account: AccountInfo; onClose: () => void; onSubmit: (apiKey: string) => Promise<void> }) {
  return (
    <Dialog open onOpenChange={close(onClose)} title="Replace key" width={420} closable>
      <ReplaceBody account={account} onClose={onClose} onSubmit={onSubmit} />
    </Dialog>
  );
}

export function ReplaceBody({ account, onClose, onSubmit }: { account: AccountInfo; onClose: () => void; onSubmit: (apiKey: string) => Promise<void> }) {
  const [apiKey, setApiKey] = useState("");
  return (
    <Form
        submit="Save key"
        canSubmit={keyReady(apiKey)}
        onCancel={onClose}
        run={async () => {
          await onSubmit(takeKey(apiKey, () => setApiKey("")));
          onClose();
        }}
    >
      <KeyField provider={account.provider} value={apiKey} onChange={setApiKey} />
    </Form>
  );
}

export function CapDialog({ account, onClose, onSubmit }: { account: AccountInfo; onClose: () => void; onSubmit: (cap: number) => Promise<void> }) {
  return (
    <Dialog open onOpenChange={close(onClose)} title="Spending cap" width={320} closable>
      <CapBody account={account} onClose={onClose} onSubmit={onSubmit} />
    </Dialog>
  );
}

export function CapBody({ account, onClose, onSubmit }: { account: AccountInfo; onClose: () => void; onSubmit: (cap: number) => Promise<void> }) {
  const [cap, setCap] = useState(account.spendingCapUsd === null ? "" : String(account.spendingCapUsd));
  const amount = parseCap(cap);
  const below = amount !== null && amount < account.spending.knownUsd ? `This is below the ${money(account.spending.knownUsd)} already spent, so bots stop using the key right away.` : null;
  return (
    <Form
        submit="Save"
        canSubmit={amount !== null}
        onCancel={onClose}
        run={async () => {
          if (amount === null) return;
          await onSubmit(amount);
          onClose();
        }}
    >
      <CapField value={cap} onChange={setCap} below={below} />
    </Form>
  );
}

export function RenameDialog({ account, onClose, onSubmit }: { account: AccountInfo; onClose: () => void; onSubmit: (label: string) => Promise<void> }) {
  return (
    <Dialog open onOpenChange={close(onClose)} title="Rename account" width={320} closable>
      <RenameBody account={account} onClose={onClose} onSubmit={onSubmit} />
    </Dialog>
  );
}

export function RenameBody({ account, onClose, onSubmit }: { account: AccountInfo; onClose: () => void; onSubmit: (label: string) => Promise<void> }) {
  const [label, setLabel] = useState(account.label);
  return (
    <Form
        submit="Save"
        canSubmit={label.trim().length > 0 && label.trim() !== account.label}
        onCancel={onClose}
        run={async () => {
          await onSubmit(label.trim());
          onClose();
        }}
    >
      <NameField value={label} onChange={setLabel} placeholder="Account name" />
    </Form>
  );
}

// An account with bots on it can't be removed: the dialog says which bots to move first. Otherwise it says what is and isn't
// removed (a sign-in the provider's own tool keeps in its folder is not claimed to be deleted).
export function RemoveDialog({ account, bots, onClose, onSubmit }: { account: AccountInfo; bots: Pick<Bot, "id" | "name" | "accountId">[]; onClose: () => void; onSubmit: () => Promise<void> }) {
  const using = botsOn(account, bots);
  if (using.length > 0) {
    return (
      <Dialog open onOpenChange={close(onClose)} title={`Can't remove ${account.label}`} width={320} closable>
        <p className="mt-2 text-[13px] leading-[18px] text-body">Move these bots to another account first:</p>
        <ul className="mt-2 flex flex-col gap-1">
          {using.map((bot) => (
            <li key={bot.id}>
              <Link href={`/bots/settings/?bot=${encodeURIComponent(bot.id)}`} className="text-[13px] text-accent hover:underline">
                {bot.name}
              </Link>
            </li>
          ))}
        </ul>
        <div className="mt-5 flex justify-end">
          <Pill primary onClick={onClose}>
            OK
          </Pill>
        </div>
      </Dialog>
    );
  }
  const what = account.kind === "api_key" ? "The key is removed from Windows Credential Manager." : "Disconnects this account from Tjommie.";
  return (
    <Dialog open onOpenChange={close(onClose)} title={`Remove ${account.label}?`} width={320} closable>
      <Form
        submit="Remove"
        danger
        canSubmit
        onCancel={onClose}
        run={async () => {
          await onSubmit();
          onClose();
        }}
      >
        <p className="mt-2 text-[13px] leading-[18px] text-body">{what} Usage history stays.</p>
      </Form>
    </Dialog>
  );
}
