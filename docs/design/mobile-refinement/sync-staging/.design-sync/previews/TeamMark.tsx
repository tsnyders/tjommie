import { TeamMark } from "@tjommie/ui-kit";

const row = { display: "flex", alignItems: "center", gap: 14, padding: 16 };

// The Team room's mark, as on its card and in the rail.
export function Default() {
  return (
    <div style={row}>
      <TeamMark />
    </div>
  );
}

export function Sizes() {
  return (
    <div style={row}>
      <TeamMark size={28} />
      <TeamMark size={40} />
      <TeamMark size={64} />
    </div>
  );
}

// Dimmed, as the rail shows it when the room isn't selected; with the room's title on the dark canvas.
export function DimmedAndDark() {
  return (
    <div className="dark" style={{ ...row, background: "var(--canvas)" }}>
      <TeamMark size={40} dimmed />
      <TeamMark size={40} />
      <span style={{ color: "var(--title)", fontSize: 15, fontWeight: 600 }}>Team room</span>
      <span style={{ color: "var(--secondary)", fontSize: 13 }}>6 bots</span>
    </div>
  );
}
