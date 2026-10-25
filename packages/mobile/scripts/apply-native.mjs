// Copies the native plugins into the generated Android project and makes the few settings changes this app needs. Safe to run again.
// Run after `cap add android` or `cap sync android`:  node scripts/apply-native.mjs
import { cpSync, existsSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const mobile = join(here, "..");
const android = join(mobile, "android");
if (!existsSync(android)) throw new Error("There is no android/ project yet: run `npm run android:add -w @tjommie/mobile` first.");

const app = join(android, "app");
const res = join(app, "src", "main", "res");
const icon = join(mobile, "..", "desktop", "assets", "icon-256.png");
mkdirSync(join(res, "drawable"), { recursive: true });
cpSync(icon, join(res, "drawable", "tjommie_logo.png"));
for (const density of readdirSync(res).filter((name) => /^mipmap-(?:m|h|xh|xxh|xxxh)dpi$/.test(name))) {
  cpSync(icon, join(res, density, "ic_launcher.png"));
  cpSync(icon, join(res, density, "ic_launcher_round.png"));
}
for (const name of ["ic_launcher.xml", "ic_launcher_round.xml"]) {
  writeFileSync(join(res, "mipmap-anydpi-v26", name), '<?xml version="1.0" encoding="utf-8"?><adaptive-icon xmlns:android="http://schemas.android.com/apk/res/android"><background android:drawable="@color/ic_launcher_background"/><foreground><inset android:drawable="@drawable/tjommie_logo" android:inset="12%"/></foreground></adaptive-icon>');
}
writeFileSync(join(res, "values", "ic_launcher_background.xml"), '<?xml version="1.0" encoding="utf-8"?><resources><color name="ic_launcher_background">#111111</color></resources>');
const javaDir = join(app, "src", "main", "java", "dev", "tjommie", "app");
cpSync(join(mobile, "native", "java", "dev", "tjommie", "app"), javaDir, { recursive: true, force: true });
cpSync(join(mobile, "native", "java-test", "dev", "tjommie", "app"), join(app, "src", "test", "java", "dev", "tjommie", "app"), { recursive: true, force: true });

function patch(file, change) {
  const before = readFileSync(file, "utf8");
  const after = change(before);
  if (after !== before) writeFileSync(file, after);
}
const must = (text, needle, what) => {
  if (!text.includes(needle)) throw new Error(`Could not find ${what} in the generated project; the template may have changed.`);
};

// The manifest: nothing in the app is backed up, and nothing travels over cleartext HTTP (the socket plugin is raw TCP, which this doesn't govern).
patch(join(app, "src", "main", "AndroidManifest.xml"), (xml) => {
  must(xml, "<application", "the <application> element");
  let out = xml;
  if (!/android:allowBackup="false"/.test(out)) out = /android:allowBackup="[^"]*"/.test(out) ? out.replace(/android:allowBackup="[^"]*"/, 'android:allowBackup="false"') : out.replace("<application", '<application\n        android:allowBackup="false"');
  if (!/android:usesCleartextTraffic=/.test(out)) out = out.replace("<application", '<application\n        android:usesCleartextTraffic="false"');
  if (!/android:windowSoftInputMode=/.test(out)) out = out.replace("<activity", '<activity\n            android:windowSoftInputMode="adjustResize"');
  return out;
});

// Keep the launch/window canvas opaque, rather than AppCompat's default grey.
// Runtime appearance follows the app's own theme via TjommieAppearance.
// Only the window gets the canvas. A theme-level android:background is inherited by every view without one of its own, the system's
// text-selection toolbar included, whose transparent popup then paints a solid band over the page; so it stays @null (and reverts if set).
patch(join(res, "values", "styles.xml"), (xml) => {
  let out = xml.replace('<item name="android:background">@color/tjommie_canvas</item>', '<item name="android:background">@null</item>');
  if (!out.includes('name="android:windowBackground"')) out = out.replace('<item name="windowNoTitle">true</item>', '<item name="windowNoTitle">true</item>\n        <item name="android:windowBackground">@color/tjommie_canvas</item>');
  return out;
});
for (const [qualifier, color] of [["values", "#fcfcfc"], ["values-night", "#070707"]]) {
  mkdirSync(join(res, qualifier), { recursive: true });
  writeFileSync(join(res, qualifier, "tjommie_canvas.xml"), `<?xml version="1.0" encoding="utf-8"?><resources><color name="tjommie_canvas">${color}</color></resources>`);
}

// The scanner and the JVM tests' dependencies.
patch(join(app, "build.gradle"), (gradle) => {
  must(gradle, "dependencies {", "the dependencies block");
  let out = gradle;
  const version = JSON.parse(readFileSync(join(mobile, "package.json"), "utf8")).version;
  out = out.replace(/versionName\s+"[^"]*"/, `versionName "${version}"`);
  if (!out.includes("play-services-code-scanner")) out = out.replace("dependencies {", "dependencies {\n    implementation 'com.google.android.gms:play-services-code-scanner:16.1.0'");
  if (!/testImplementation\s+['"]junit:junit/.test(out)) out = out.replace("dependencies {", "dependencies {\n    testImplementation 'junit:junit:4.13.2'");
  return out;
});
console.log("Native plugins applied to", android);
