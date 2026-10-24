import { Ellipsis, PanelRight, Search, Settings } from "lucide-react";
import { IconButton } from "@tjommie/ui-kit";

const row = { display: "flex", alignItems: "center", gap: 4, padding: 16 };

export function Default() {
  return (
    <div style={row}>
      <IconButton label="Search bots">
        <Search aria-hidden />
      </IconButton>
    </div>
  );
}

// Active, as the panel button looks while its panel is open.
export function Active() {
  return (
    <div style={row}>
      <IconButton label="Bot panel" active>
        <PanelRight aria-hidden />
      </IconButton>
    </div>
  );
}

// A header's trailing actions.
export function HeaderActions() {
  return (
    <div style={row}>
      <IconButton label="Search bots">
        <Search aria-hidden />
      </IconButton>
      <IconButton label="Bots settings">
        <Settings aria-hidden />
      </IconButton>
      <IconButton label="More">
        <Ellipsis aria-hidden />
      </IconButton>
    </div>
  );
}

export function OnDark() {
  return (
    <div className="dark" style={{ ...row, background: "var(--canvas)" }}>
      <IconButton label="Search bots">
        <Search aria-hidden />
      </IconButton>
      <IconButton label="Bot panel" active>
        <PanelRight aria-hidden />
      </IconButton>
      <IconButton label="More">
        <Ellipsis aria-hidden />
      </IconButton>
    </div>
  );
}
