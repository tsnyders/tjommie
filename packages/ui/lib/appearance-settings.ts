export type TextSize = "system" | "large" | "larger";
export type Density = "comfortable" | "compact";
const TEXT = "tjommie.textSize";
const DENSITY = "tjommie.density";

function stored(key: string): string | null {
  try { return localStorage.getItem(key); } catch { return null; }
}

export function textSize(): TextSize {
  const value = stored(TEXT);
  return value === "large" || value === "larger" ? value : "system";
}

export function density(): Density { return stored(DENSITY) === "compact" ? "compact" : "comfortable"; }

export function showAppearance(): void {
  document.documentElement.dataset.textSize = textSize();
  document.documentElement.dataset.density = density();
}

export function applyTextSize(value: TextSize): void { localStorage.setItem(TEXT, value); showAppearance(); }
export function applyDensity(value: Density): void { localStorage.setItem(DENSITY, value); showAppearance(); }

export const appearanceScript = `var t=localStorage.getItem("${TEXT}");document.documentElement.dataset.textSize=t==="large"||t==="larger"?t:"system";document.documentElement.dataset.density=localStorage.getItem("${DENSITY}")==="compact"?"compact":"comfortable"`;
