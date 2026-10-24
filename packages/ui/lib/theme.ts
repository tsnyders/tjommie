import { desktop } from "./desktop";
import { setNativeCanvas } from "./mobile";
import { hexColor, mobileStyleScript, saveMobileStyle, saveReduceEffects, showMobileStyle, type MobileStyle } from "./mobile-style";

export type ThemeChoice = "dark" | "light" | "system";
const KEY = "tjommie.theme";

// Runs in <head> before the first paint, so the page never flashes the wrong theme (or phone style). Dark is the default.
export const themeScript = `(function(){try{${mobileStyleScript};var c=localStorage.getItem("${KEY}");var light=c==="light"||(c==="system"&&matchMedia("(prefers-color-scheme: light)").matches);if(!light)document.documentElement.classList.add("dark")}catch(e){document.documentElement.classList.add("dark")}})()`;

export function themeChoice(): ThemeChoice {
  const stored = localStorage.getItem(KEY);
  return stored === "light" || stored === "system" ? stored : "dark";
}

const resolved = (choice: ThemeChoice): "dark" | "light" =>
  choice === "system" ? (matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark") : choice;

// Saves the choice, recolors the page, and tells the desktop app so the window's caption buttons match.
export function applyTheme(choice: ThemeChoice): void {
  localStorage.setItem(KEY, choice);
  const theme = resolved(choice);
  document.documentElement.classList.toggle("dark", theme === "dark");
  desktop()?.setTheme(theme);
  syncNativeCanvas();
}

// Tells the Android app the page's canvas colour, so its system bars and the window behind the page match (a no-op anywhere else).
export function syncNativeCanvas(): void {
  const color = hexColor(getComputedStyle(document.documentElement).getPropertyValue("--canvas"));
  if (color) setNativeCanvas({ color, dark: document.documentElement.classList.contains("dark") });
}

// The phone's style and its "Reduce effects" switch change the canvas too.
export function applyMobileStyle(style: MobileStyle): void {
  saveMobileStyle(style);
  syncNativeCanvas();
}

export function applyReduceEffects(on: boolean): void {
  saveReduceEffects(on);
  syncNativeCanvas();
}

// Applies the saved choice now, and again when Windows switches between light and dark while it's "system".
export function followTheme(): () => void {
  showMobileStyle();
  applyTheme(themeChoice());
  const media = matchMedia("(prefers-color-scheme: light)");
  const onChange = (): void => {
    if (themeChoice() === "system") applyTheme("system");
  };
  media.addEventListener("change", onChange);
  return () => media.removeEventListener("change", onChange);
}
