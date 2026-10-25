import { BAR_PILL, Pill } from "@tjommie/ui-kit";

const row = { display: "flex", alignItems: "center", gap: 8, flexWrap: "wrap" as const, padding: 16 };

// An approval card's answers: the main action is the light pill.
export function Approval() {
  return (
    <div style={row}>
      <Pill primary>Allow once</Pill>
      <Pill>Always allow</Pill>
      <Pill>Deny</Pill>
    </div>
  );
}

// Locked after one tap while the daemon records the answer.
export function Disabled() {
  return (
    <div style={row}>
      <Pill primary disabled>
        Allow once
      </Pill>
      <Pill disabled>Deny</Pill>
    </div>
  );
}

// The larger size of a page's action bar (New bot, Bot settings).
export function ActionBar() {
  return (
    <div style={{ ...row, justifyContent: "flex-end" }}>
      <Pill className={BAR_PILL}>Cancel</Pill>
      <Pill primary className={BAR_PILL}>
        Save
      </Pill>
    </div>
  );
}

export function OnDark() {
  return (
    <div className="dark" style={{ ...row, background: "var(--canvas)" }}>
      <Pill primary>Allow once</Pill>
      <Pill>Always allow</Pill>
      <Pill>Deny</Pill>
    </div>
  );
}
