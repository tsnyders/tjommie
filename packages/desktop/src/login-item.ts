import { existsSync, mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";

type LoginApp = {
  isPackaged: boolean;
  getPath(name: "userData"): string;
  getLoginItemSettings(options: { args: string[] }): { openAtLogin: boolean; launchItems: { args: string[]; enabled: boolean }[] };
  setLoginItemSettings(options: { openAtLogin: boolean; args: string[] }): void;
};

const args = ["--hidden"];

// NSIS relaunches an updated app with --updated; the restart stays in the tray.
export const startsHidden = (argv: readonly string[]): boolean => argv.includes("--hidden") || argv.includes("--updated");

export function syncLoginItem(app: LoginApp): void {
  if (!app.isPackaged) return;
  const root = app.getPath("userData");
  const marker = join(root, "login-item-set");
  if (existsSync(marker)) return;
  app.setLoginItemSettings({ openAtLogin: true, args });
  mkdirSync(root, { recursive: true });
  writeFileSync(marker, "");
}

export function getLoginItem(app: LoginApp): boolean {
  if (!app.isPackaged) return false;
  const settings = app.getLoginItemSettings({ args });
  return settings.openAtLogin && settings.launchItems.some((item) => item.enabled && item.args.includes("--hidden"));
}

export function setLoginItem(app: LoginApp, on: boolean): void {
  if (app.isPackaged) app.setLoginItemSettings({ openAtLogin: on, args });
}
