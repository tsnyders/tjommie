# UI bug hunt and mobile motion

The original Quiet pill layout, colours and navigation are retained. UI fixes cover both layouts. New animation rules apply only below 640 px, with no new desktop motion.

## Fixed interactions

- Chat, Team room and exchange sends retain edits made while their RPC is pending, including erasing and retyping the same text. A synchronous guard prevents duplicate sends in one frame.
- Chat attachments have stable identities. Send completion removes only sent attachments. A file picker that returns later reads current attachments and cannot restore a sent or removed file.
- Team room keyboard selection scrolls only the mention list to keep the selected bot visible. Escape targeted at the listbox by Android Back dismisses it.
- A canceled routine editor's late Save/Delete completion cannot close a newer editor.
- Failed device revocation remains visible in the confirmation, with a retry and a pending guard, instead of silently closing the dialog.
- Usage refreshes and invalidates stale reports at local midnight, zone changes and resume. The latest request wins if refreshes overlap.

## Mobile motion

- Quiet pill and segmented selection colours: 120 ms.
- Menus and account/model popovers: 160 ms opacity/scale transitions, 120 ms exit.
- Dialogs: restrained phone scale of .98, 200 ms entry, 120 ms exit.
- Details, Members and Exchange sheets: 200 ms opacity/scale .99→1 entrance. The Details native-browser container is excluded.
- No streamed-message, typing, scrolling or layout-height animations. Reduced motion disables all additions.

## Verification

- Independent source reviewer cleared the accepted bug fixes and mobile-only motion scope.
- UI suite: 57 files, 415 tests passed. Root/UI TypeScript, UI eslint and explicit-any check passed (358 TypeScript files).
- `ui-bughunt.real.test.ts` passed in the actual production renderer with a disposable daemon: delayed chat/room replies, a delayed native picker, newer routine editor, mention keyboard visibility, native-equivalent overlay Back, failed then successful revoke, and calendar rollover.
- `mobile-motion.real.test.ts` passed: real animation sampled midway remains inside the viewport; phone menu timing, reduced-motion and unchanged desktop computed styles verified.
- Android package verified all 72 exported UI assets. APK SHA-256: `f627c891ee382cef1d6d0b3d842d87056fe18c94e7cb331fa9c2d801eb3e07a6`.

All fixture accounts were unsigned and bots paused. No provider turn, real-account sign-in or owner-file attachment was used. A first physical run stopped on a harness race that treated the connecting screen's pairing form as the final connection state; the harness now waits for saved-PC connection before switching.

The final physical S25 run passed in 70.31 seconds, including real swipe scrolling, bot creation, a long Team room draft, Android Back dismissing mentions, chat/approval, keyboard bounds, mobile animation selection, Details/system Back, Usage, saved-key reconnect and revocation. An intermediate launch also hit a stale WebView inspection endpoint; the final run starts the app cleanly and verifies its diagnostic endpoint before attaching.

Both package records use source-input hash `69bb3fea5b2f3170e60e7b8b6605b638e6ec24719da164b3914b86e9c4617a76`. The mandatory Windows packaged smoke passed. Windows installer SHA-256: `2677260775cacf26066b8d8709844d9a5c2eb2bbf4531e9005fc5d57ba6b7a40`.

Both updates are installed. All 570 installed Windows payload files matched the tested package; the phone's installed APK hash matched its package record. Before the desktop restart, its work queue had zero running items. The desktop service restarted successfully and the S25 restored its saved connection. Physical fixture pairings and temporary profiles were cleaned up; the owner's existing pairing remains.
