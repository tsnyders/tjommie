import { BotAvatar } from "@tjommie/ui-kit";

const row = { display: "flex", alignItems: "center", gap: 12, flexWrap: "wrap" as const, padding: 16 };

// Every shape a bot can have, each in a bot colour.
export function Shapes() {
  return (
    <div style={row}>
      <BotAvatar shape="circle" color="blue" />
      <BotAvatar shape="blob" color="pink" />
      <BotAvatar shape="rounded-square" color="green" />
      <BotAvatar shape="pill" color="purple" />
      <BotAvatar shape="triangle" color="red" />
      <BotAvatar shape="hexagon" color="orange" />
      <BotAvatar shape="cloud" color="teal" />
      <BotAvatar shape="droplet" color="amber" />
    </div>
  );
}

// The sizes the app uses: mention list, chat header, agent rows, cards and the bot panel.
export function Sizes() {
  return (
    <div style={row}>
      {[18, 24, 28, 40, 56].map((size) => (
        <BotAvatar key={size} shape="hexagon" color="orange" size={size} />
      ))}
    </div>
  );
}

// As it sits in an agent row: avatar, name, state.
export function InARow() {
  return (
    <div style={{ ...row, gap: 10, background: "var(--canvas)" }}>
      <BotAvatar shape="cloud" color="pink" size={28} />
      <div>
        <div style={{ color: "var(--title)", fontSize: 15, fontWeight: 600, lineHeight: "20px" }}>Inbox Triage</div>
        <div style={{ color: "var(--working)", fontSize: 13, lineHeight: "18px" }}>Working</div>
      </div>
    </div>
  );
}

// The same avatars on the dark canvas.
export function OnDark() {
  return (
    <div className="dark" style={{ ...row, background: "var(--canvas)" }}>
      <BotAvatar shape="hexagon" color="orange" />
      <BotAvatar shape="cloud" color="pink" />
      <BotAvatar shape="rounded-square" color="green" />
      <BotAvatar shape="circle" color="blue" />
    </div>
  );
}
