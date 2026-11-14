import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

// Converts the Aegis design tokens (design/claude-design/aegis/tokens.json, W3C design tokens) into the desktop theme
// file tjommie-rs/design/themes/aegis.json: one theme-neutral semantic key per token, with lengths in px and times in
// ms as plain numbers. Later themes (Workbench, Mission Control) fill the same keys (spec Phase 0), which
// crates/desktop/tests/theme_keys.rs checks. After changing the tokens: npm run theme:aegis.

type Json = string | number | boolean | null | Json[] | { [key: string]: Json };
type JsonObject = { [key: string]: Json };

// The per-mode colours by role, so another theme can fill the same keys. Every Aegis colour has a line; a new colour
// without one fails the conversion.
const COLOR_ROLES: Record<string, string> = {
  bg: "surface.background", paper: "surface.paper", white: "surface.card", frame: "surface.frame",
  panel: "surface.panel", raised: "surface.raised", "raised-clear": "surface.raisedClear", sunk: "surface.sunk",
  text: "text.primary", "text-2": "text.secondary", "text-3": "text.tertiary",
  ink: "ink.base", "ink-mid": "ink.mid", "ink-muted": "ink.muted", "ink-hover": "ink.hover",
  icon: "icon.base", "icon-muted": "icon.muted",
  line: "line.base", "line-soft": "line.soft", "line-field": "line.field",
  accent: "accent.base", "accent-text": "accent.text", "accent-tint": "accent.tint", marker: "accent.marker",
  primary: "action.primary", "on-primary": "on.primary", "on-ink": "on.ink", "on-accent": "on.accent",
  hover: "state.hover", "sunk-hover": "state.sunkHover", "panel-hover": "state.panelHover",
  pressed: "state.pressed", focus: "state.focus",
  scrim: "overlay.scrim", highlight: "overlay.highlight",
  "avatar-eye": "avatar.eye", sweep: "avatar.sweep",
  // The Tjommie mark: a body gradient from start to end, with eyes.
  "stone-1": "brand.markStart", "stone-2": "brand.markEnd", "stone-eye": "brand.markEye",
};

const fail = (path: string, why: string): never => { throw new Error(`${path}: ${why}`); };
const isObject = (value: unknown): value is Record<string, unknown> => typeof value === "object" && value !== null && !Array.isArray(value);
const camel = (key: string) => key.replace(/-(\w)/g, (_, letter: string) => letter.toUpperCase());

function num(path: string, value: unknown): number {
  return typeof value === "number" && Number.isFinite(value) ? value : fail(path, `expected a number, got ${JSON.stringify(value)}`);
}

// "24px" -> 24; a bare "0" is a length too (CSS).
function measure(path: string, value: unknown, unit: "px" | "ms" | "em" | "%"): number {
  const match = typeof value === "string" ? /^(-?\d*\.?\d+)(px|ms|em|%)?$/.exec(value) : null;
  const ok = match !== null && (match[2] === unit || (match[2] === undefined && unit === "px" && Number(match[1]) === 0));
  return ok ? Number(match[1]) : fail(path, `expected ${unit}, got ${JSON.stringify(value)}`);
}

// "0 10px 28px -6px #3C3C3A24, 0 2px 6px #3C3C3A0F" -> one { x, y, blur, spread, color } per layer.
function shadow(path: string, value: unknown): Json {
  if (typeof value !== "string") return fail(path, "expected a shadow string");
  return value.split(",").map((layer) => {
    const parts = layer.trim().split(/\s+/), color = parts.pop() ?? "";
    const [x, y, blur = "0", spread = "0", ...rest] = parts;
    if (!/^#[0-9A-F]{6}([0-9A-F]{2})?$/i.test(color) || rest.length > 0) fail(path, `unreadable shadow layer "${layer}"`);
    return { x: measure(path, x, "px"), y: measure(path, y, "px"), blur: measure(path, blur, "px"), spread: measure(path, spread, "px"), color };
  });
}

function typography(path: string, value: unknown): Json {
  if (!isObject(value)) return fail(path, "expected a typography object");
  const font = /^\{font\.(\w+)\}$/.exec(String(value.fontFamily))?.[1] ?? fail(path, "fontFamily must reference font.*");
  return {
    font, weight: num(path, value.fontWeight), size: measure(path, value.fontSize, "px"),
    lineHeight: measure(path, value.lineHeight, "px"), letterSpacingEm: measure(path, value.letterSpacing, "em"),
    widthPercent: measure(path, value.fontWidth, "%"),
  };
}

function convert(path: string, token: Record<string, unknown>): Json {
  const value = token.$value;
  switch (token.$type) {
    case "color": return typeof value === "string" && /^#[0-9A-F]{6}([0-9A-F]{2})?$/i.test(value) ? value : fail(path, "expected #rrggbb or #rrggbbaa");
    case "fontFamily": return Array.isArray(value) && value.every((family) => typeof family === "string") ? value : fail(path, "expected font family names");
    case "typography": return typography(path, value);
    case "dimension": return measure(path, value, "px");
    case "duration": return measure(path, value, "ms");
    case "number": return num(path, value);
    case "shadow": return shadow(path, value);
    case "cubicBezier": return Array.isArray(value) && value.length === 4 ? value.map((point) => num(path, point)) : fail(path, "expected four numbers");
    // The spring keeps what the motion runtime reads; mass is 1, and damping and settleMs follow from these.
    case "spring": return isObject(value) ? { stiffness: num(path, value.stiffness), dampingRatio: num(path, value.dampingRatio) } : fail(path, "expected a spring");
    case "object": return isObject(value) ? Object.fromEntries(Object.entries(value).map(([key, field]) => [key, num(path, field)])) : fail(path, "expected an object");
    default: return fail(path, `unknown token type ${JSON.stringify(token.$type)}`);
  }
}

function semanticKey(path: string[]): string {
  const [group, mode, name = ""] = path;
  if (group === "color" && (mode === "light" || mode === "dark")) return `color.${mode}.${COLOR_ROLES[name] ?? fail(path.join("."), "no semantic role; add it to COLOR_ROLES")}`;
  if (group === "space") return `space.s${mode}`;
  return path.map(camel).join(".");
}

function set(theme: JsonObject, key: string, value: Json): void {
  const names = key.split("."), last = names.pop() ?? "";
  let node = theme;
  for (const name of names) {
    const next = node[name] ?? (node[name] = {});
    node = isObject(next) ? next as JsonObject : fail(key, "collides with another key");
  }
  if (last in node) fail(key, "two tokens map to this key");
  node[last] = value;
}

/** The theme and its keys, one per token, in token order. */
export function aegisTheme(tokens: unknown): { theme: JsonObject; keys: string[] } {
  const theme: JsonObject = {}, keys: string[] = [];
  const walk = (node: unknown, path: string[]): void => {
    if (!isObject(node)) return;
    if ("$value" in node) {
      const key = semanticKey(path);
      set(theme, key, convert(path.join("."), node));
      keys.push(key);
      return;
    }
    for (const [name, child] of Object.entries(node)) if (!name.startsWith("$")) walk(child, [...path, name]);
  };
  walk(tokens, []);
  return { theme, keys };
}

export function aegisThemeText(tokens: unknown): string {
  return `${JSON.stringify(aegisTheme(tokens).theme, null, 2)}\n`;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const args = process.argv.slice(2);
  const check = args[0] === "--check";
  const file = resolve(args[check ? 1 : 0] ?? fileURLToPath(new URL("../tjommie-rs/design/themes/aegis.json", import.meta.url)));
  const tokens: unknown = JSON.parse(readFileSync(new URL("../design/claude-design/aegis/tokens.json", import.meta.url), "utf8"));
  const text = aegisThemeText(tokens);
  if (check) {
    let current = "";
    try { current = readFileSync(file, "utf8").replaceAll("\r\n", "\n"); } catch { /* A missing file is stale. */ }
    if (current !== text) {
      console.error(`${file} is stale. Run npm run theme:aegis and commit the result.`);
      process.exitCode = 1;
    }
  } else {
    mkdirSync(dirname(file), { recursive: true });
    writeFileSync(file, text);
  }
}
