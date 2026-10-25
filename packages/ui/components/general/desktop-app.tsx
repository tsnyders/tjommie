"use client";

import { useEffect, useState } from "react";
import { desktop, type UpdateState } from "@/lib/desktop";
import { errorText } from "@/lib/labels";
import { Switch } from "../controls";
import { Pill } from "../pill";

const LOGIN = "Start Tjommie when you sign in to Windows";

function updateText(update: UpdateState): string {
  switch (update.status) {
    case "idle":
      return "Not checked yet";
    case "checking":
      return "Checking";
    case "none":
      return "Up to date";
    case "downloading":
      return `Downloading ${Math.round(update.percent)}%`;
    case "ready":
      return `Ready: Restart to update to v${update.version}`;
    case "error":
      return `Couldn't check: ${update.message}`;
    case "unsupported":
      return "This build doesn't update itself.";
  }
}

const couldntCheck = (error: unknown): UpdateState => ({ status: "error", message: errorText(error) });

// The desktop app's start-at-sign-in and updater, or null outside it and in a desktop app from before them.
const appControls = () => {
  const api = desktop();
  return api?.loginItem && api.updates ? api : null;
};

// Settings → General in the desktop app (this PC's app, whichever PC the window is on): starting at sign-in, and updates. Read when General opens;
// the updater then says each change. The switch shows what the desktop app last confirmed, and a call that fails says why.
export function DesktopApp() {
  const [login, setLogin] = useState<boolean | null>(null);
  const [loginProblem, setLoginProblem] = useState<string | null>(null);
  const [update, setUpdate] = useState<UpdateState | null>(null);
  useEffect(() => {
    const api = appControls();
    if (!api) return;
    void api.loginItem.get().then(setLogin, (error: unknown) => setLoginProblem(`Couldn't read this setting: ${errorText(error)}`));
    void api.updates.state().then(setUpdate, (error: unknown) => setUpdate(couldntCheck(error)));
    return api.updates.onState(setUpdate);
  }, []);
  const api = appControls();
  if (!api) return null;
  const busy = update?.status === "checking" || update?.status === "downloading";
  const setLoginItem = (on: boolean): void => {
    setLoginProblem(null);
    void api.loginItem.set(on).then(
      () => setLogin(on),
      (error: unknown) => setLoginProblem(`Couldn't change this setting: ${errorText(error)}`),
    );
  };
  return (
    <section className="mt-4 flex max-w-[700px] flex-col divide-y divide-divider rounded-[12px] bg-card px-4">
      <div className="flex min-h-14 items-center justify-between gap-4 py-3">
        <div className="min-w-0">
          <p className="text-[14px] text-text">{LOGIN}</p>
          {loginProblem && (
            <p role="alert" className="mt-0.5 text-[12px] leading-[18px] text-danger [overflow-wrap:anywhere]">
              {loginProblem}
            </p>
          )}
        </div>
        <Switch label={LOGIN} checked={login ?? false} disabled={login === null} onChange={setLoginItem} />
      </div>
      {update && (
        <div className="flex min-h-14 items-center justify-between gap-4 py-3">
          <div className="min-w-0">
            <p className="text-[14px] text-text">Updates</p>
            <p className="mt-0.5 text-[12px] leading-[18px] text-secondary [overflow-wrap:anywhere]">{updateText(update)}</p>
          </div>
          {update.status === "ready" ? (
            <Pill primary onClick={() => api.updates.restart()}>
              Restart
            </Pill>
          ) : (
            update.status !== "unsupported" && (
              <Pill disabled={busy} onClick={() => void api.updates.check().catch((error: unknown) => setUpdate(couldntCheck(error)))}>
                Check now
              </Pill>
            )
          )}
        </div>
      )}
    </section>
  );
}
