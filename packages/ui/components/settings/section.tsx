import type { ReactNode } from "react";

export function SettingsSection({ title, note, children }: { title: string; note: string; children: ReactNode }) {
  return <div className="max-w-[700px] [overflow-wrap:anywhere]">
    <h2 className="text-[20px] font-semibold leading-[26px] text-title">{title}</h2>
    <p className="mt-2 text-[13px] leading-[18px] text-secondary">{note}</p>
    {children}
  </div>;
}

export function SettingsCard({ children }: { children: ReactNode }) {
  return <section data-slot="settings-card" className="mt-4 flex min-w-0 flex-col divide-y divide-divider rounded-[12px] bg-card px-4">{children}</section>;
}

export function SettingsRow({ title, note, children }: { title: string; note?: string; children?: ReactNode }) {
  return <div data-slot="settings-row" className="flex min-h-14 flex-wrap items-center justify-between gap-x-4 gap-y-2 py-4">
    <div className="min-w-0 flex-1 basis-40">
      <p className="text-[14px] text-text">{title}</p>
      {note && <p className="mt-1 text-[12px] leading-[18px] text-secondary">{note}</p>}
    </div>
    {children}
  </div>;
}

export function SettingsProblem({ children }: { children: ReactNode }) {
  return <p role="alert" className="mt-3 text-[12px] leading-[18px] text-danger">{children}</p>;
}
