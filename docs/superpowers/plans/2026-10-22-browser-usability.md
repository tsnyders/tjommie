# Browser usability and responsiveness follow-up

Date: 2026-10-22. Branch: `browser-usability-20261022`, based on `1b64f36`.

Owner report: Google rejects sign-in in the embedded browser; control is laggy; the details/routines/activity panel is difficult to return to; browser width and an expanded view are needed. Use the Paseo UI agent for UI planning. Never use `any` as a TypeScript type.

## Intended behavior

- Left bot rail has consistent spacing between selected/hovered buttons, without reducing hit targets or clipping focus rings (additional owner screenshot request).
- Persistent Details / Browser tabs in one panel shell. Details stays reachable after opening a browser. Keep the browser pane mounted once opened, retaining tab and address-draft state.
- Selecting Details releases held input and returns control to the bot. Hidden browser panes stop rendering/polling. Closing/hiding the panel retains the existing safe handback and context lifetime.
- Accessible drag splitter, keyboard sizing and saved width, clamped to preserve usable chat space. Narrow windows use an overlay rather than crush the chat.
- Expand browser into the application's content area; Restore returns to the prior width. Keep rail and window controls available. Expanded/overlay content has correct focus and inert background behavior. Escape must respect page dialogs and address editing.
- Keep panel controls outside the native top-right 138×44 caption area. Expanded and overlay modes inert only the chat, preserving rail and native window controls.
- Reversible suspend must hand back after an in-flight takeover, and a quick re-show must wait for that barrier.
- Captured pointer releases still reach the page when a drag ends outside the image or a resize clears the frame.
- Browser viewport fills available width and height. Preserve image proportions and reject clicks in letterbox gutters; map rendered pixels to actual page viewport coordinates. Resize uses bounded coalescing and existing stale-image fences.
- Separate one bounded frame-read lane from ordered input/control operations. Fence stale state/frames across control, page, generation and layout changes. Preserve ordered keys/buttons and close teardown, with no raw debugging endpoint.
- Measure input-to-DOM and decoded-display latency using disposable local pages before and after the change. Coalesce high-frequency operations without reordering click/key boundaries. Do not claim gains solely from an interval change.

## Google sign-in boundary

The screenshot is a real sign-in failure. Google may reject automated or embedded sign-in contexts. Native rendering, user-agent spoofing and hiding automation flags are not established Google-login fixes. Do not copy credentials or profiles, weaken browser security, or call Google authentication verified without an owner-controlled test.

The owner later considered one-time system-browser OAuth and full API access to Gmail, Drive and Calendar, then clarified that actual websites and services such as YouTube must also work inside the browser. After inspecting Paseo, the owner approved the native desktop-browser migration in `2026-10-22-native-browser.md`. That is the active implementation. The separate API experiment is preserved but disabled in production and absent from the bot panel.

Sources:
- https://support.google.com/accounts/answer/7675428?hl=en
- https://developers.googleblog.com/en/guidance-to-developers-affected-by-our-effort-to-block-less-secure-browsers-and-applications/

## Ownership

- Paseo UI Builder: chat-screen/header/bot-panel/browser-panel, new right-panel, panel-layout and browser-geometry helpers/tests.
- Paseo Deep Investigation: BrowserSession and its tests; daemon browser manager/frame changes and focused real fixture; additive browser protocol metadata if required.
- Coordinator: this plan, integration, desktop real UI acceptance, documentation, combined verification and delivery.
- Reviewers: report findings only; no file edits or builds/tests.

## Acceptance gates

- [ ] Details and Browser switching restores the correct pane and state; no hidden human lock remains.
- [ ] Pointer/keyboard splitter, persistence, narrow-window overlay, expand/restore and focus are tested in the real desktop.
- [ ] Exact image/page click alignment at narrow, tall and expanded sizes; no stale resize/navigation frames.
- [ ] Modal answers, takeover/handback, tab changes, hide/reopen and browser disable retain existing safety.
- [ ] Measured input/display responsiveness and bounded queues; slow frame does not block input.
- [ ] Independent read-only review and focused/full tests, no-any, TypeScript, lint/build.
- [ ] Fresh package/provenance and installed tests before calling the update delivered.
- [ ] Record Google authentication as unresolved unless a supported real-site path is separately verified.

Google website sign-in remains a separate acceptance gate for the installed native browser. Synthetic cookies and local media tests do not establish Google compatibility.

Do not interrupt or replace the installed app while it may be in use. No push or merge is included in this follow-up.
