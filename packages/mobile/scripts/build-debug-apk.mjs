// Builds a debug-signed APK with the Android SDK on this PC. It is installable for testing only: there is no release key.
import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const android = join(dirname(fileURLToPath(import.meta.url)), "..", "android");
if (!existsSync(android)) throw new Error("There is no android/ project yet: run `npm run android:add -w @tjommie/mobile` first.");
const wrapper = join(android, process.platform === "win32" ? "gradlew.bat" : "gradlew");
const result = spawnSync(wrapper, ["assembleDebug"], { cwd: android, stdio: "inherit", shell: process.platform === "win32" });
if (result.status !== 0) process.exit(result.status ?? 1);
console.log("APK: packages/mobile/android/app/build/outputs/apk/debug/app-debug.apk");
