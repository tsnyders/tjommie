import { useState } from "react";
import { Stepper } from "@tjommie/ui-kit";

function ChainLimit({ initial }: { initial: number }) {
  const [value, setValue] = useState(initial);
  return (
    <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: 16, padding: "10px 16px", maxWidth: 380 }}>
      <span style={{ color: "var(--text)", fontSize: 14 }}>Requests between bots per chain</span>
      <Stepper label="Requests between bots per chain" value={value} min={1} max={10} onChange={setValue} />
    </div>
  );
}

// Settings, Bots: the chain limit.
export function ChainLimitSetting() {
  return <ChainLimit initial={3} />;
}

// At its lower bound the minus is aria-disabled, so focus stays put.
export function AtMinimum() {
  return <ChainLimit initial={1} />;
}
