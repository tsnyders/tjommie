// Builds an Android APK. Release signing is checked by Gradle and the packaging script.
import { execFileSync, execSync } from "node:child_process";
import { existsSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const android = join(dirname(fileURLToPath(import.meta.url)), "..", "android");
if (!existsSync(android)) throw new Error("There is no android/ project yet: run `npm run android:add -w @tjommie/mobile` first.");
const release = process.argv[2] === "--release";
if (process.argv.length > 3 || (process.argv.length === 3 && !release)) throw new Error("Use android:debug with optional --release only.");
const wrapper = join(android, process.platform === "win32" ? "gradlew.bat" : "gradlew");
const task = release ? "assembleRelease" : "assembleDebug";
if (process.platform === "win32") execSync(`"${wrapper}" ${task}`, { cwd: android, stdio: "inherit", windowsHide: true });
else execFileSync(wrapper, [task], { cwd: android, stdio: "inherit", windowsHide: true });
console.log(`APK: packages/mobile/android/app/build/outputs/apk/${release ? "release/app-release.apk" : "debug/app-debug.apk"}`);
