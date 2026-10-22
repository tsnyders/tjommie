import { mkdtemp, mkdir, readFile, realpath, rm, symlink, unlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { Session } from "electron";
import { afterEach, expect, it, vi } from "vitest";
import { boundedProfileCleanup, checkedNativeProfile, clearNativeProfile, nativeProfileDirectories, removeNativeProfile } from "./native-browser-profiles.ts";

vi.mock("node:fs/promises", async (original) => {
  const actual = await original<typeof import("node:fs/promises")>();
  return { ...actual, realpath: vi.fn(actual.realpath), rm: vi.fn(actual.rm) };
});
afterEach(() => { vi.mocked(realpath).mockReset(); vi.mocked(rm).mockClear(); vi.useRealTimers(); });

it("only enumerates hashed profiles and removes the checked owned child", async () => {
  const root = await mkdtemp(join(tmpdir(), "native-profile-policy-")), hash = "a".repeat(64);
  try {
    await mkdir(join(root, hash)); await mkdir(join(root, "not-a-profile"));
    await writeFile(join(root, hash, "cookie-test"), "synthetic");
    expect(await nativeProfileDirectories(root)).toEqual([hash]);
    expect(await removeNativeProfile(root, hash)).toBe(true);
    expect(await checkedNativeProfile(root, hash)).toBeNull();
    expect(await checkedNativeProfile(root, "../outside").catch(() => "rejected")).toBe("rejected");
  } finally { await rm(root, { recursive: true, force: true }); }
});

it.skipIf(process.platform !== "win32")("accepts a differently cased Windows root and canonical child identity", async () => {
  const root = await mkdtemp(join(tmpdir(), "native-profile-Windows-")), hash = "d".repeat(64);
  try {
    await mkdir(join(root, hash));
    const canonicalRoot = await realpath(root), canonicalChild = await realpath(join(root, hash));
    const suppliedRoot = root.toUpperCase();
    expect(suppliedRoot).not.toBe(canonicalRoot);
    expect(await nativeProfileDirectories(suppliedRoot)).toEqual([hash]);
    expect(await checkedNativeProfile(suppliedRoot, hash)).toBe(canonicalChild);
    // Windows may normalize the parent's spelling differently between root and child calls.
    vi.mocked(realpath).mockResolvedValueOnce(canonicalRoot).mockResolvedValueOnce(canonicalChild.toUpperCase());
    expect(await checkedNativeProfile(suppliedRoot, hash)).toBe(canonicalChild.toUpperCase());
    expect(await removeNativeProfile(suppliedRoot, hash)).toBe(true);
    expect(await checkedNativeProfile(root, hash)).toBeNull();
  } finally { await rm(root, { recursive: true, force: true }); }
});

it("rejects canonical children outside the root and a changed target before deletion", async () => {
  const root = await mkdtemp(join(tmpdir(), "native-profile-canonical-")), hash = "e".repeat(64);
  try {
    await mkdir(join(root, hash));
    const canonicalRoot = await realpath(root), child = join(canonicalRoot, hash);
    vi.mocked(realpath).mockResolvedValueOnce(canonicalRoot).mockResolvedValueOnce(join(canonicalRoot, "outside", hash));
    await expect(checkedNativeProfile(root, hash)).rejects.toThrow(/escaped/);
    vi.mocked(realpath).mockResolvedValueOnce(canonicalRoot).mockResolvedValueOnce(join(canonicalRoot, "f".repeat(64)));
    await expect(checkedNativeProfile(root, hash)).rejects.toThrow(/escaped/);
    const differentBase = join(canonicalRoot, "redirected");
    await mkdir(join(differentBase, hash), { recursive: true });
    // Both individual resolutions are contained, but changing the base during the final
    // recheck must still prevent deletion of the earlier target.
    vi.mocked(realpath).mockResolvedValueOnce(canonicalRoot).mockResolvedValueOnce(child)
      .mockResolvedValueOnce(differentBase).mockResolvedValueOnce(join(differentBase, hash));
    await expect(removeNativeProfile(root, hash)).rejects.toThrow(/changed during/);
    expect(rm).not.toHaveBeenCalled();
  } finally { await rm(root, { recursive: true, force: true }); }
});

it("refuses root, profile and nested junctions without touching their destination", async () => {
  const root = await mkdtemp(join(tmpdir(), "native-profile-links-")), hash = "b".repeat(64);
  const owned = join(root, "owned"), outside = join(root, "outside"), alias = join(root, "alias");
  await mkdir(owned); await mkdir(outside); await writeFile(join(outside, "keep"), "outside");
  try {
    await symlink(outside, alias, "junction");
    await expect(nativeProfileDirectories(alias)).rejects.toThrow(/physical/);
    await symlink(outside, join(owned, hash), "junction");
    await expect(removeNativeProfile(owned, hash)).rejects.toThrow(/physical/);
    await unlink(join(owned, hash)); await mkdir(join(owned, hash));
    await symlink(outside, join(owned, hash, "nested"), "junction");
    await expect(removeNativeProfile(owned, hash)).rejects.toThrow(/symbolic/);
    expect(await readFile(join(outside, "keep"), "utf8")).toBe("outside");
    await unlink(join(owned, hash, "nested")); await unlink(alias);
  } finally { await rm(root, { recursive: true, force: true }); }
});

it("defers a Windows file lock without retries and surfaces unexpected removal errors", async () => {
  const root = await mkdtemp(join(tmpdir(), "native-profile-locked-")), hash = "c".repeat(64);
  try {
    await mkdir(join(root, hash));
    vi.mocked(rm).mockRejectedValueOnce(Object.assign(new Error("locked"), { code: "EBUSY" }));
    expect(await removeNativeProfile(root, hash)).toBe(false);
    expect(rm).toHaveBeenLastCalledWith(join(root, hash), { recursive: true, force: true, maxRetries: 0 });
    vi.mocked(rm).mockRejectedValueOnce(Object.assign(new Error("I/O failure"), { code: "EIO" }));
    await expect(removeNativeProfile(root, hash)).rejects.toThrow("I/O failure");
  } finally { await rm(root, { recursive: true, force: true }); }
});

it("clears storage, cached authentication and connections, then requires durably empty cookies", async () => {
  const cookies = { flushStore: vi.fn(async () => {}), get: vi.fn(async () => [] as { name: string }[]) };
  const profile = { cookies, clearStorageData: vi.fn(async () => {}), clearCache: vi.fn(async () => {}), clearAuthCache: vi.fn(async () => {}), closeAllConnections: vi.fn(async () => {}), flushStorageData: vi.fn() };
  await clearNativeProfile(profile as unknown as Session);
  expect(profile.clearStorageData).toHaveBeenCalled(); expect(profile.clearAuthCache).toHaveBeenCalled();
  expect(profile.clearCache).toHaveBeenCalled(); expect(profile.closeAllConnections).toHaveBeenCalled();
  expect(cookies.flushStore).toHaveBeenCalled(); expect(cookies.get).toHaveBeenCalledWith({});
  cookies.get.mockResolvedValueOnce([{ name: "remaining" }]);
  await expect(clearNativeProfile(profile as unknown as Session)).rejects.toThrow("still contains cookies");
  profile.clearStorageData.mockRejectedValueOnce(new Error("clear failed"));
  await expect(clearNativeProfile(profile as unknown as Session)).rejects.toThrow("clear failed");
});

it("fails closed within a bounded time when Chromium cleanup never answers", async () => {
  vi.useFakeTimers();
  const cleanup = boundedProfileCleanup(new Promise<void>(() => {}), "test", 50);
  const assertion = expect(cleanup).rejects.toThrow("timed out: test");
  await vi.advanceTimersByTimeAsync(50); await assertion;
});
