import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { Stepper } from "./controls";

describe("Stepper", () => {
  it("keeps a button at its bound focusable, marked aria-disabled instead of disabled", () => {
    const html = renderToStaticMarkup(<Stepper label="Chain limit" value={1} min={1} max={20} onChange={() => {}} />);
    expect(html).toContain('aria-label="Decrease" aria-disabled="true"');
    expect(html).toContain('aria-label="Increase" aria-disabled="false"');
    expect(html).not.toContain("disabled=\"\"");
    // ...and doesn't light up on hover, as a disabled button didn't.
    expect(html).toContain("aria-disabled:hover:bg-transparent aria-disabled:hover:text-icon");
  });
});
