import { expect, it } from "vitest";
import { patchFirebaseDependency, patchFirebaseVersion } from "./firebase-dependency.ts";

it("replaces a changed Firebase version without duplicate app dependencies", () => {
  const initial = "dependencies {\n    implementation 'com.google.firebase:firebase-messaging:25.0.1'\n}";
  const first = patchFirebaseDependency(initial, "25.0.1");
  const second = patchFirebaseDependency(first, "26.0.0");
  expect(second.match(/^\s*implementation ['"]com\.google\.firebase:firebase-messaging:[^\r\n]+$/gm)).toEqual([
    '    implementation "com.google.firebase:firebase-messaging:26.0.0"',
  ]);
});

it("replaces the shared Firebase version on a second patch", () => {
  const first = patchFirebaseVersion("ext {\n    minSdkVersion = 24\n}", "25.0.1");
  const second = patchFirebaseVersion(first, "26.0.0");
  expect(second.match(/^\s*firebaseMessagingVersion\s*=.*$/gm)).toEqual(["    firebaseMessagingVersion = '26.0.0'"]);
});
