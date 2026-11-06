## What and why

<!-- What does this change, and why is it needed? Link the issue. -->

## How it was tested

<!-- Tests added or run, platforms tried, screenshots for UI changes. -->

## Checklist

- [ ] The title follows Conventional Commits (`feat(workspace): …`, `fix(ui): …`).
- [ ] `cargo xtask ci` passes locally.
- [ ] No synchronous I/O or heavy work on the UI thread.
- [ ] New UI strings are in both `assets/locales/en` and `assets/locales/pl`.
- [ ] New user-facing features are actions in the `ferro_keymap` catalog.
- [ ] Any `unsafe` block has a `// SAFETY:` comment (needs a second reviewer).
- [ ] Performance-sensitive change: spike/benchmark numbers attached.
