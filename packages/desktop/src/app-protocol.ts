import { existsSync, statSync } from "node:fs";
import { basename, dirname, extname, isAbsolute, join, normalize, relative } from "node:path";

export const DEV_ORIGIN = "http://localhost:3000";

// The static export uses trailingSlash, so "/chat/" is "chat/index.html". Next's page data requests
// ("/chat/index.txt?_rsc=...") are plain files; the query string is ignored.
export function uiFile(uiDir: string, url: string): string | null {
  let path: string;
  try {
    const target = new URL(url);
    if (target.protocol !== "app:" || target.host !== "tjommie") return null;
    path = decodeURIComponent(target.pathname);
  } catch {
    return null;
  }
  if (path.endsWith("/")) path += "index.html";
  else if (!extname(path)) path += "/index.html";
  const file = normalize(join(uiDir, path));
  const inside = relative(uiDir, file);
  if (inside.startsWith("..") || isAbsolute(inside)) return null;
  if (existsSync(file)) return statSync(file).isFile() ? file : null;

  // The client flattens segment paths with dots, but the Windows export keeps nested folders.
  const parts = basename(file).split(".");
  const segments = parts.slice(1, -1);
  if (parts[0] !== "__next" || parts[parts.length - 1] !== "txt" || segments.length < 2) return null;
  if (segments.some((segment) => !/^[A-Za-z0-9_!$@-]+$/.test(segment))) return null;
  const alternate = normalize(
    join(dirname(file), `__next.${segments[0]}`, ...segments.slice(1, -1), `${segments[segments.length - 1]}.txt`),
  );
  const alternateInside = relative(uiDir, alternate);
  if (alternateInside.startsWith("..") || isAbsolute(alternateInside)) return null;
  return existsSync(alternate) && statSync(alternate).isFile() ? alternate : null;
}

// Only Tjommie's own pages load in the window. Web links open in the default browser.
export function navigationFor(url: string, production: boolean): "stay" | "browser" | "block" {
  let target: URL;
  try {
    target = new URL(url);
  } catch {
    return "block";
  }
  const inApp = production ? target.protocol === "app:" && target.host === "tjommie" : target.origin === DEV_ORIGIN;
  if (inApp) return "stay";
  return target.protocol === "https:" || target.protocol === "http:" ? "browser" : "block";
}
