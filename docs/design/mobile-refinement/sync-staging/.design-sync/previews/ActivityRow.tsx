import { ActivityRow } from "@tjommie/ui-kit";

// One line per turn of tool work in a chat; a tap expands it to one row per step.
export function FinishedTurn() {
  return (
    <div style={{ padding: 16, maxWidth: 600 }}>
      <ActivityRow
        summary="5 steps: Tagged v2.4.0-rc.1"
        items={[
          { id: "1", name: "Bash", summary: "Pulled main", status: "done" },
          { id: "2", name: "Bash", summary: "Ran npm test: 212 passed", status: "done" },
          { id: "3", name: "Bash", summary: "Built all packages", status: "done" },
          { id: "4", name: "Read", summary: "Checked the changelog", status: "done" },
          { id: "5", name: "Bash", summary: "Tagged v2.4.0-rc.1", status: "done" },
        ]}
      />
    </div>
  );
}

export function RunningTurn() {
  return (
    <div style={{ padding: 16, maxWidth: 600 }}>
      <ActivityRow
        summary="3 steps: Running npm publish --dry-run"
        items={[
          { id: "1", name: "Bash", summary: "Checked the npm login", status: "done" },
          { id: "2", name: "Read", summary: "Read package.json", status: "done" },
          { id: "3", name: "Bash", summary: "Running npm publish --dry-run", status: "running" },
        ]}
      />
    </div>
  );
}

export function OnDark() {
  return (
    <div className="dark" style={{ padding: 16, background: "var(--canvas)" }}>
      <ActivityRow
        summary="2 steps: Sorted 38 new emails"
        items={[
          { id: "1", name: "Gmail", summary: "Read 38 new emails", status: "done" },
          { id: "2", name: "Gmail", summary: "Sorted 38 new emails", status: "done" },
        ]}
      />
    </div>
  );
}
