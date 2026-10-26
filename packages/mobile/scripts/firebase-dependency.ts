export function patchFirebaseVersion(gradle: string, version: string): string {
  if (!gradle.includes("ext {")) throw new Error("Could not find Android version settings.");
  const newline = gradle.includes("\r\n") ? "\r\n" : "\n";
  const without = gradle.replace(/^[ \t]*firebaseMessagingVersion\s*=.*(?:\r?\n|$)/gm, "");
  return without.replace("ext {", `ext {${newline}    firebaseMessagingVersion = '${version}'`);
}

export function patchFirebaseDependency(gradle: string, version: string): string {
  if (!gradle.includes("dependencies {")) throw new Error("Could not find Android app dependencies.");
  const newline = gradle.includes("\r\n") ? "\r\n" : "\n";
  const without = gradle.replace(/^[ \t]*implementation\s+['"]com\.google\.firebase:firebase-messaging:[^\r\n]*(?:\r?\n|$)/gm, "");
  return without.replace("dependencies {", `dependencies {${newline}    implementation "com.google.firebase:firebase-messaging:${version}"`);
}
