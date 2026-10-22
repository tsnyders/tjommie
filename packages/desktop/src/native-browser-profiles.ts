import { lstat, readdir, realpath, rm } from "node:fs/promises";
import { basename, dirname, join, resolve } from "node:path";
import type { Session } from "electron";

export const nativeProfileHash = /^[0-9a-f]{64}$/;
function code(error: unknown): string | undefined {
  return error && typeof error === "object" && "code" in error && typeof error.code === "string" ? error.code : undefined;
}
function samePath(left: string, right: string): boolean {
  return process.platform === "win32" ? left.toLowerCase() === right.toLowerCase() : left === right;
}

async function profileRoot(root: string): Promise<string | null> {
  const absolute = resolve(root);
  try {
    const info = await lstat(absolute);
    if (!info.isDirectory() || info.isSymbolicLink()) throw new Error("Browser profile root must be an owned physical directory.");
    return await realpath(absolute);
  } catch (error) { if (code(error) === "ENOENT") return null; throw error; }
}

export async function nativeProfileDirectories(root: string): Promise<string[]> {
  const base = await profileRoot(root);
  if (!base) return [];
  return (await readdir(base, { withFileTypes: true })).filter((entry) => nativeProfileHash.test(entry.name) && (entry.isDirectory() || entry.isSymbolicLink())).map((entry) => entry.name);
}

export async function checkedNativeProfile(root: string, hash: string): Promise<string | null> {
  if (!nativeProfileHash.test(hash)) throw new Error("Invalid browser profile directory.");
  const base = await profileRoot(root);
  if (!base) return null;
  const path = resolve(base, hash);
  if (!samePath(dirname(path), base)) throw new Error("Browser profile escaped its owned root.");
  try {
    const info = await lstat(path);
    if (!info.isDirectory() || info.isSymbolicLink()) throw new Error("Browser profile must be an owned physical directory.");
    const canonical = await realpath(path);
    if (!samePath(dirname(canonical), base) || !samePath(basename(canonical), hash)) throw new Error("Browser profile escaped its owned root.");
    return canonical;
  } catch (error) { if (code(error) === "ENOENT") return null; throw error; }
}

async function rejectLinks(path: string): Promise<void> {
  for (const entry of await readdir(path, { withFileTypes: true })) {
    if (entry.isSymbolicLink()) throw new Error("Browser profile cleanup refuses symbolic links.");
    if (entry.isDirectory()) await rejectLinks(join(path, entry.name));
  }
}

/** Never follow links or retry Windows locks. A cleared, locked profile is retried at next startup. */
export async function removeNativeProfile(root: string, hash: string): Promise<boolean> {
  const path = await checkedNativeProfile(root, hash);
  if (!path) return true;
  await rejectLinks(path);
  // Recheck the direct target immediately before recursive deletion.
  const current = await checkedNativeProfile(root, hash);
  if (!current || !samePath(current, path)) throw new Error("Browser profile changed during cleanup.");
  try { await rm(path, { recursive: true, force: true, maxRetries: 0 }); return true; }
  catch (error) { if (["EBUSY", "EPERM", "EACCES", "ENOTEMPTY"].includes(code(error) ?? "")) return false; throw error; }
}

export async function boundedProfileCleanup<T>(work: Promise<T>, label: string, timeout = 8000): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  try { return await Promise.race([work, new Promise<never>((_resolve, reject) => { timer = setTimeout(() => reject(new Error(`Browser profile cleanup timed out: ${label}`)), timeout); })]); }
  finally { clearTimeout(timer); }
}

/** Fail closed if Chromium cannot clear or durably flush authentication state. */
export async function clearNativeProfile(profile: Session): Promise<void> {
  await boundedProfileCleanup((async () => {
    await Promise.all([profile.clearStorageData(), profile.clearCache(), profile.clearAuthCache(), profile.closeAllConnections()]);
    profile.flushStorageData();
    await profile.cookies.flushStore();
    if ((await profile.cookies.get({})).length) throw new Error("Browser profile still contains cookies after clearing.");
  })(), "clear storage and cookies");
}
