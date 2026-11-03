import type { Session, WebContents } from "electron";

export function speechOrigin(value: string, production: boolean): boolean {
  try {
    const url = new URL(value);
    if (url.username || url.password) return false;
    return production ? url.protocol === "app:" && url.hostname === "tjommie" && !url.port : url.origin === "http://localhost:3000";
  } catch { return false; }
}

export function installSpeechPermissions(profile: Pick<Session, "setPermissionCheckHandler" | "setPermissionRequestHandler">, appWindow: () => WebContents | null, capturing: () => boolean, production: boolean): void {
  const trusted = (contents: WebContents | null): boolean => contents !== null && contents === appWindow() && !contents.isDestroyed() && capturing() && speechOrigin(contents.getURL(), production);
  profile.setPermissionCheckHandler((contents, permission, origin, details) => trusted(contents) && permission === "media" && details.isMainFrame === true && details.mediaType === "audio" && speechOrigin(origin, production) && typeof details.requestingUrl === "string" && speechOrigin(details.requestingUrl, production));
  profile.setPermissionRequestHandler((contents, permission, callback, details) => callback(trusted(contents) && permission === "media" && details.isMainFrame === true && speechOrigin(details.requestingUrl, production) && "mediaTypes" in details && Array.isArray(details.mediaTypes) && details.mediaTypes.length === 1 && details.mediaTypes[0] === "audio"));
}
