import { useState } from "react";
import { Segmented } from "@tjommie/ui-kit";

// Usage's range.
export function Range() {
  const [value, setValue] = useState("week");
  const options = [
    { value: "today", label: "Today" },
    { value: "week", label: "This week" },
    { value: "month", label: "This month" },
    { value: "year", label: "This year" },
  ];
  return (
    <div style={{ padding: 16 }}>
      <Segmented label="Range" value={value} options={options} onChange={setValue} />
    </div>
  );
}

// Settings, General: Appearance.
export function Appearance() {
  const [value, setValue] = useState("dark");
  const options = [
    { value: "dark", label: "Dark" },
    { value: "light", label: "Light" },
    { value: "system", label: "System" },
  ];
  return (
    <div style={{ padding: 16 }}>
      <Segmented label="Appearance" value={value} options={options} onChange={setValue} />
    </div>
  );
}

// Full width, splitting its container in equal parts (the model picker's provider).
export function FullWidth() {
  const [value, setValue] = useState("claude");
  const options = [
    { value: "claude", label: "Claude" },
    { value: "codex", label: "Codex" },
  ];
  return (
    <div style={{ padding: 16, maxWidth: 360 }}>
      <Segmented label="Provider" value={value} options={options} onChange={setValue} full />
    </div>
  );
}
