import { MissingAvatar } from "@tjommie/ui-kit";

// Where a deleted bot's avatar would be, beside the app's own wording for it.
export function InAPost() {
  return (
    <div style={{ display: "flex", alignItems: "center", gap: 8, padding: 16 }}>
      <MissingAvatar size={18} />
      <span style={{ color: "var(--title)", fontSize: 13, fontWeight: 600 }}>A deleted bot</span>
      <span style={{ color: "var(--caption)", fontSize: 12 }}>to Night Shift</span>
    </div>
  );
}

export function Sizes() {
  return (
    <div style={{ display: "flex", alignItems: "center", gap: 14, padding: 16 }}>
      <MissingAvatar size={14} />
      <MissingAvatar size={18} />
      <MissingAvatar size={28} />
      <MissingAvatar size={40} />
    </div>
  );
}
