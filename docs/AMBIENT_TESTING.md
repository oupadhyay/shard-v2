# Ambient integration: testing guide

Use a test profile or back up Shard's app data before testing deletion. Use only
test memories, conversations, and routines. Do not test side effects on important
files or accounts.

## Start the real app

Check out the PR branch, then run from the repository root:

```sh
npm ci
npm test -- --run
npm run build
npm run tauri dev
```

Use the Tauri app, not just the browser page. On macOS, allow screen recording
only if you want to test screen capture. Add a provider key in Settings for the
live checks. Never include keys in a bug report.

## Main checks

1. **Capture and restore.** Start with empty chat. Expect a quiet Pierre Dark
   composer. Type a draft, attach an image, hide with Escape, and restore with
   Ctrl+Space. Check text and attachment retention. An open menu or view consumes
   Escape before the panel hides. Back closes a capability view and currently
   discards unsaved form edits.
2. **Conversation.** Ask for a long answer with a Markdown table and a list.
   Expect fixed 360px width, vertical growth, capped inner scrolling, and local
   horizontal table scrolling. Scroll up while output arrives: the app should
   not pull you to the end. Stop, then resend. Confirm no duplicated user turn.
3. **Context.** Capture a harmless screen. Inspect the preview, remove it, then
   capture again and send. Check that only the selected context is sent and that
   the sent snapshot does not change when the next draft changes.
4. **History.** Create two test conversations. Search for each by title or
   summary, open it, and confirm the right messages load. Keep an unsent draft
   and attachment while switching. Delete an inactive test conversation, then
   an active one. Each deletion requires confirmation; no unrelated chat is lost.
5. **Saved memory.** Ask Shard to save a harmless preference. Open Saved memory,
   correct it, and Undo. Forget it and Undo. Restart and confirm the saved state.
   Status and Undo share one compact row. No success should appear before the
   host confirms it. Forget does not remove source conversations or all inferred
   observations. A stale correction or Undo must fail, not overwrite newer data.
6. **Routines.** Create a harmless routine with a five-field cron expression
   such as `*/5 * * * *`. Check its next occurrence, edit it, and pause it. Restart:
   it must stay paused. Resume and verify one live run, then delete the test
   routine. Changes can take about two seconds to reach the scheduler. Pause and
   delete do not cancel a turn that has already started.
7. **Trust.** Ask for a persona change. Inspect the exact text before approval;
   reject one proposal and approve another. Check the saved content. Approval
   alone must not mean success. Failed or unknown actions remain visible in Needs
   attention, with no automatic replay. Repeat after restart. Do not create an
   unknown outcome by interrupting a real important action.
8. **Privacy and failure.** Check the Reduce Automatic Memory disclosure. History
   is still retained. Disconnect the network and send a harmless question: expect
   an error, not a success claim or silent replay. Restore the connection.
9. **Native macOS.** Try bright and busy wallpapers, reduced motion/transparency,
   another display, Spaces, focus changes, and repeated global shortcuts. Open
   the dedicated compatibility view and return. Check typing focus, clipping,
   screen-capture permissions, and window placement.

## Automated backend checks

From `src-tauri`:

```sh
cargo test --workspace
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
```

## Review evidence (Linux orb, September 8, 2026)

- `npm test -- --run`: 185 passed, clean exit. `npm run build`: passed.
- `cargo test --workspace`: 549 passed, including the old-deletion Undo
  regression. All-target check, targeted rustfmt, and strict rustdoc passed.
- Strict Clippy failed on the unchanged `vector_store.rs:727` lint below.
- Real Tauri/WebKit with the production frontend under Xvfb/Openbox, in a
  separate test profile: seeded memory correction and Undo changed the real
  JSON store; seeded history search/load/delete worked and retained the draft;
  routine creation and pause worked; pause survived a full restart.
- Escape hid the native window; Ctrl+Space restored the exact draft and Send.
  Memory and paused-routine screenshots were inspected. Native DOM checks found
  360px viewport and 360px document width. These were real IPC checks using test
  data, not live provider generation.
- Earlier implementation-thread GUI checks also covered 320px width, bright
  wallpaper, screen capture preview/removal, and memory status-row stability.
  They were not all repeated in this final review.

## Screenshot feedback follow-up (September 10, 2026)

- Frontend: 185 tests passed and build passed. Backend: 549 tests and the
  all-target check passed. Focused wire tests cover attention context inclusion
  in normal chat and exclusion under Reduce Automatic Memory.
- Linux native render/IPC checks: inline Settings and Save, 12px privacy help,
  collapsed long routine, flat 84px history rows, and reviewed unknown action
  without old approval-request wording. All inspected views fit 360px width.
- MacBook native Tauri/WKWebView rendering was inspected in an isolated profile:
  inline Settings, eight collapsed routines, flat populated history, collapsed
  attention, and expanded unknown action. Controls fit and no horizontal overflow
  appeared. Navigation was triggered programmatically for captures, not through
  native input. The tested revision was the initial feedback patch plus the final
  header gap; later action-context changes were tested separately in the orb.
  Mac targeted checks passed: 65 frontend, 32 heartbeat, and 5 agent-process
  tests, plus the frontend build. No real app keys or data were used.
- Mac input remains blocked by Accessibility/Automation permission (post/listen
  access false). Ctrl+Space, Escape, focus/draft retention, and physical clicks on
  Save/Close/Back and routine controls remain untested on Mac. Spaces and live
  provider behavior also remain unverified.
- Needs attention starts collapsed. Unknown records are retained; there is still
  no user-resolution workflow that can safely remove them from the list.

## Known limits

- No new first-run scope grants or durable offline queue are implemented.
- Memory JSON and migrated SQLite retrieval updates are separate commits; a
  partial sync failure needs follow-up. Routine file writers do not all share
  one concurrency lock. See TODO for these limits.
- The existing strict Clippy failure is `chunks_exact_to_as_chunks` in
  `src/vector_store.rs`. An earlier frontend run in the implementation thread
  hit late diff-viewer timer errors; later clean runs do not prove it is fixed.
- Linux native checks cannot verify macOS behavior. Live provider, external-tool,
  and OCR-provider checks need credentials. A packaged build is still required
  before release.

When reporting a problem, include the OS, PR revision, steps, expected result,
actual result, and a redacted screenshot or log. Say whether you used the native
app, a browser preview, or a simulated fixture.
