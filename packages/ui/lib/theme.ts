import { desktop } from "./desktop";

export type ThemeChoice = "dark" | "light" | "system";
const KEY = "tjommie.theme";

// Runs in <head> before the first paint, so the page never flashes the wrong theme. Dark is the default.
export const themeScript = `(function(){try{var c=localStorage.getItem("${KEY}");var light=c==="light"||(c==="system"&&matchMedia("(prefers-color-scheme: light)").matches);if(!light)document.documentElement.classList.add("dark")}catch(e){document.documentElement.classList.add("dark")}})()`;

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
}

// Applies the saved choice now, and again when Windows switches between light and dark while it's "system".
export function followTheme(): () => void {
  applyTheme(themeChoice());
  const media = matchMedia("(prefers-color-scheme: light)");
  const onChange = (): void => {
    if (themeChoice() === "system") applyTheme("system");
  };
  media.addEventListener("change", onChange);
  return () => media.removeEventListener("change", onChange);
}
