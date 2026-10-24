# Windows and Android test delivery

Built on 2026-10-22 from `phase2-accounts-mobile-20261022`. These are local test artifacts; the APK uses Android debug signing. No release signing key was created. The first delivery hashes below are historical; the Android responsiveness update at the end supersedes that APK.

## Artifacts

| Artifact | Bytes | SHA-256 |
| --- | ---: | --- |
| `dist-desktop/installer/Tjommie Setup 0.1.0.exe` | 120611337 | `8737bd3aa135aa4fa81c75030713f0bfa864596f98f5e50d2e05e8139e72c59d` |
| `dist-mobile/Tjommie Android 0.1.0 debug.apk` | 5706418 | `0c77f860e5be7bba18aadf22136985e51968dd1541b1b08c84d915eec99c4d51` |

Both provenance records match source-input hash `e65a9d0ec90419207ce17100821fb916ecf8bfab14b074392757e18971f64163`. Android packaging verified all 72 exported UI assets inside the APK. `apksigner verify --verbose` passed using APK Signature Scheme v2.

## Completed checks

- Root/UI typechecks, no-explicit-any check (351 TypeScript files), UI eslint: passed.
- Default fast suite: 130 files, 1,524 tests passed.
- Android JVM suite: 12 tests, no failures or errors, including frame bounds and exported document routing.
- Mandatory `npm run test:packaged`: one test passed, with source and every packaged artifact checked before and after execution.
- Packaged `phone-layout.real.test.ts`: one passed, including 360/390 px, sheets, forms, Usage and narrow native browser visibility.
- Packaged `embedded-browser.real.test.ts`: one passed.
- Packaged `accounts.real.test.ts`: one passed.
- Physical S25 Ultra, Android 17, source PC fixture: one end-to-end test passed. Actual encrypted Wi-Fi pairing, Devices registration, chat submission, synthetic approval, real keyboard/composer bounds, Details/system Back, Usage/year estimate, reload/saved-key reconnect and immediate revocation were exercised.
- The same complete physical S25 test against `dist-desktop/installer/win-unpacked/Tjommie.exe`: one passed in 48.25 seconds. This confirms the delivered Windows payload and installed APK work together.

The phone test exposed and fixed a native routing issue: Capacitor's SPA fallback served the home document when reloading Settings. Known exported document routes now use their own bundled HTML through Capacitor's existing bridge injection. A physical reload of Usage passes. A frame-codec test was updated to expect early protocol rejection of an oversized claim after the frame limit was reduced to 65,535 bytes.

## Test data and limits

All test PC profiles, bot messages, approvals and costs were disposable and synthetic. The paused test bot never started a provider turn. No owner credentials, sign-in, logout or paid calls were used. Successful physical QR scanning and live owner-provider quota responses are not claimed; physical pairing used the pasted code.

The APK is installed on the S25. Test pairings are revoked and forgotten. The existing installed desktop session was not stopped or replaced; the matching Windows installer is delivered for the owner to run.

Same-network use only: PC awake with Tjommie running, TCP 47101 reachable. No relay/mobile-data connection or push notifications. Native browser viewing/control and file picking remain desktop-only. Each paired device has full owner authority. Snow is maintained but explicitly unaudited upstream; native wrapping protects the saved Noise key, which temporarily enters JS/WASM memory during connection.

## Original Quiet UI responsiveness update

The owner requested keeping the original Quiet pill design. The temporary alternate mobile UI was removed. These changes preserve the existing components, colours and navigation:

- The phone shell and connection content have bounded flex heights, allowing nested vertical scrolling.
- The avatar preview stacks above the identity fields; colour and shape choices wrap inside the phone width.
- New bot and Bot settings keep their own scrollers, explicit phone padding and reachable action footers. Expanded row controls use the available width.
- Settings navigation scrolls on short screens. Limited-account banners wrap their actions and long unbroken labels.
- The Team room mention menu scrolls above the composer without adding layout height. A phone draft has a viewport-relative height cap.

The UI Builder implemented the scoped layout fixes and the independent source reviewer cleared them. Root/UI typechecks and the no-explicit-any check passed (353 TypeScript files). The focused final layout tests passed: 14 tests in 4 files. The earlier complete fast run passed 1,531 tests in 132 files, before the final two small overflow refinements.

The complete authorized S25 Ultra test passed against an isolated instance of the installed Windows executable. It used actual ADB touch swipes in both directions, created a bot through the original form, scrolled Team room mentions and a 35-message chat, answered a synthetic approval, checked Send above the real keyboard, exercised Details/system Back, Usage/year estimates, saved-key reconnect and revocation. Accounts were unsigned and bots paused; no provider turn was started. The owner's running desktop session and saved phone identity were preserved.

The final repeat also passed (74.06 seconds), including a 15-line Team room draft with the mention menu and real keyboard open. The updated APK is installed on the S25. Its device-side SHA-256, local artifact and provenance match: `d62ea4d31b83ca6970eaf9311d5762c75f23d154a2e803ad5c39e66b0626d652`. Source-input hash: `c4dbe9ed4a056f7416b35de189e86626508d0eafa72164d6094704986de899fe`. Packaging verified all 72 exported UI assets inside the APK. This round did not replace the Windows installation.
