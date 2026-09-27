import type { RoutineLabelData } from "@tjommie/protocol";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { RoutineLabel } from "./notices";

const label = (status: RoutineLabelData["status"], note: string | null) =>
  renderToStaticMarkup(<RoutineLabel text="Routine: Pnet apply 9am" data={{ routineId: "r1", runId: "u1", status, note }} />);

describe("RoutineLabel", () => {
  it("says why a run was missed or skipped", () => {
    expect(label("done", null)).toContain(">Routine: Pnet apply 9am</span>");
    expect(label("missed", null)).toContain(">Routine: Pnet apply 9am · Missed, the PC was off</span>");
    expect(label("skipped", "Pnet apply was paused.")).toContain(">Routine: Pnet apply 9am · Skipped: Pnet apply was paused</span>");
    expect(label("skipped", null)).toContain(">Routine: Pnet apply 9am · Skipped</span>");
    expect(label("skipped", "")).toContain(">Routine: Pnet apply 9am · Skipped</span>");
  });
});
