import { existsSync, statSync } from "node:fs";
import { extname, isAbsolute, join, normalize, relative } from "node:path";

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
  return existsSync(file) && statSync(file).isFile() ? file : null;
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
