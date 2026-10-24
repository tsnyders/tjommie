// The phone's look: Hush (the default), Folio or Aurora, kept per device. It only changes the layout below 640 px; the desktop never reads it.
// The styles themselves are CSS in globals.css, keyed on html[data-mobile-style]. "Reduce effects" swaps Aurora's glass and drifting light for
// solid surfaces.
export type MobileStyle = "hush" | "folio" | "aurora";

export const MOBILE_STYLES = [
  { value: "hush", label: "Hush" },
  { value: "folio", label: "Folio" },
  { value: "aurora", label: "Aurora" },
] as const satisfies readonly { value: MobileStyle; label: string }[];

export const STYLE_KEY = "tjommie.mobileStyle";
export const EFFECTS_KEY = "tjommie.reduceEffects";

const isStyle = (value: string | null): value is MobileStyle => MOBILE_STYLES.some((style) => style.value === value);

export function mobileStyle(): MobileStyle {
  try {
    const stored = localStorage.getItem(STYLE_KEY);
    return isStyle(stored) ? stored : "hush";
  } catch {
    return "hush";
  }
}

export function reduceEffects(): boolean {
  try {
    return localStorage.getItem(EFFECTS_KEY) === "1";
  } catch {
    return false;
  }
}

// Puts the stored choices on <html>. The theme script does the same before the first paint, so there is no flash.
export function showMobileStyle(): void {
  const root = document.documentElement;
  root.dataset.mobileStyle = mobileStyle();
  root.toggleAttribute("data-reduce-effects", reduceEffects());
}

export function saveMobileStyle(style: MobileStyle): void {
  localStorage.setItem(STYLE_KEY, style);
  showMobileStyle();
}

export function saveReduceEffects(on: boolean): void {
  if (on) localStorage.setItem(EFFECTS_KEY, "1");
  else localStorage.removeItem(EFFECTS_KEY);
  showMobileStyle();
}

// The before-paint half of showMobileStyle, for the theme script.
export const mobileStyleScript = `var s=localStorage.getItem("${STYLE_KEY}");document.documentElement.dataset.mobileStyle=s==="folio"||s==="aurora"?s:"hush";if(localStorage.getItem("${EFFECTS_KEY}")==="1")document.documentElement.setAttribute("data-reduce-effects","")`;

// A CSS colour as #rrggbb, for the native canvas (it accepts nothing else): #rgb, #rrggbb, #rrggbbaa and rgb()/rgba() are read; anything else is null.
export function hexColor(value: string): string | null {
  const text = value.trim().toLowerCase();
  const short = /^#([0-9a-f])([0-9a-f])([0-9a-f])$/.exec(text);
  if (short) return `#${short[1]}${short[1]}${short[2]}${short[2]}${short[3]}${short[3]}`;
  const long = /^#([0-9a-f]{6})(?:[0-9a-f]{2})?$/.exec(text);
  if (long) return `#${long[1]}`;
  const rgb = /^rgba?\(\s*(\d+)[\s,]+(\d+)[\s,]+(\d+)/.exec(text);
  if (!rgb) return null;
  return `#${rgb
    .slice(1, 4)
    .map((part) => Math.min(255, Number(part)).toString(16).padStart(2, "0"))
    .join("")}`;
}
