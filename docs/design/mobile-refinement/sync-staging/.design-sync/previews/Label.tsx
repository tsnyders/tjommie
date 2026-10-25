import { FIELD, Label } from "@tjommie/ui-kit";

// A form field as New bot draws it: the label over a FIELD-styled input.
export function WithField() {
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 6, padding: 16, maxWidth: 360 }}>
      <Label htmlFor="bot-name">Name</Label>
      <input id="bot-name" className={FIELD} style={{ height: 44 }} defaultValue="Night Shift" />
    </div>
  );
}

export function WithPlaceholder() {
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 6, padding: 16, maxWidth: 360 }}>
      <Label htmlFor="bot-job">What should it do?</Label>
      <input id="bot-job" className={FIELD} style={{ height: 44 }} placeholder="Run the release checklist every night" />
    </div>
  );
}
