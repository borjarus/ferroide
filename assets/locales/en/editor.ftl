## FerroIDE — Editor view layer: gutter, breadcrumbs, sticky lines, inlay hints, popups.
## Every message here must also exist in pl/editor.ftl (checked by tests).

editor-breadcrumb-tooltip = Go to this symbol
editor-problem-count = ✕ { $errors }  ⚠ { $warnings }
editor-toolbar-search = Find in file
editor-toolbar-soft-wrap = Wrap
editor-toolbar-inlay-hints = Hints
editor-run-main = Run or debug main
editor-run-test = Run or debug { $name }
editor-status-indent = { $size } spaces
editor-status-problems = Errors and warnings in this file

## Files: saving, encodings, line separators, changes on disk (Stage 3).
editor-modified = Modified
editor-saved = Saved { $file }
editor-save-failed = Could not save { $file }: { $error }
editor-save-unmappable = { $encoding } cannot represent “{ $char }” ({ $count } character(s)). Choose another encoding in the status bar.
editor-read-only = Read-only
editor-binary-file = This file looks binary.
editor-open-anyway = Open as Text
editor-large-file = Large file: syntax highlighting and code analysis are off.
editor-huge-file = Very large file: showing the first { $mb } MB, read-only.
editor-decode-errors = Some bytes are not valid { $encoding } and show as �. Saving would replace them.
editor-mixed-line-endings = The file mixes line separators; saving uses { $ending }.
editor-changed-on-disk = { $file } changed on disk.
editor-changed-on-disk-dirty = { $file } changed on disk and has unsaved changes here.
editor-reload = Reload
editor-compare = Compare
editor-keep-mine = Keep Mine
editor-deleted-on-disk = { $file } was deleted on disk. Saving will create it again.
editor-reloaded = { $file } reloaded from disk.
editor-diff-disk = Disk
editor-diff-editor = Editor
status-line-separator-menu = Line separator for this file
status-encoding-reopen = Reopen with Encoding
status-encoding-save = Save with Encoding
