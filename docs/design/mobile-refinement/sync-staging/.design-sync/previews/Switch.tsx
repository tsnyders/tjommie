import { useState } from "react";
import { Switch } from "@tjommie/ui-kit";

function Setting({ label, initial, disabled = false }: { label: string; initial: boolean; disabled?: boolean }) {
  const [on, setOn] = useState(initial);
  return (
    <label style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: 16, padding: "10px 16px", maxWidth: 360, color: "var(--text)", fontSize: 14 }}>
      {label}
      <Switch label={label} checked={on} onChange={setOn} disabled={disabled} />
    </label>
  );
}

export function On() {
  return <Setting label="Use the browser" initial />;
}

export function Off() {
  return <Setting label="Run on a schedule" initial={false} />;
}

export function Disabled() {
  return <Setting label="Run on a schedule" initial={false} disabled />;
}
