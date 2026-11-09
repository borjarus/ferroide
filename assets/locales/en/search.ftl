## FerroIDE — Quick Open, Command Palette and Find in Files.
## Every message here must also exist in pl/search.ftl (checked by tests).

## Quick Open / Command Palette
search-query-label = Quick Open
search-results-label = Results
search-placeholder-files = Search files by name
search-placeholder-commands = Type a command name
search-placeholder-symbols-file = Type a symbol name in this file
search-placeholder-symbols-workspace = Type a symbol name in the project
search-placeholder-line = Type a line number, optionally line:column
search-placeholder-text = Type text to search in the project
search-mode-files = Files
search-mode-commands = Commands
search-mode-symbols-file = Symbols in file
search-mode-symbols-workspace = Workspace symbols
search-mode-line = Go to line
search-mode-text = Text search
search-hint = > commands · @ symbols in file · # workspace symbols · : line · % text
search-loading = Searching…
search-no-files = No matching files
search-no-commands = No matching commands
search-no-symbols = No matching symbols
search-no-text = No matches
search-no-editor = Open a file in the editor to use this mode
search-type-to-search = Type to search
search-line-hint = Type a line number between 1 and { $lines }; line:column also works
search-line-invalid = “{ $input }” is not a line number
search-line-out-of-range = Line { $line } is outside the file (1–{ $lines })
search-line-target = Go to line { $line }, column { $column }
search-recent = recently opened
search-recent-command = recently used
search-preview-loading = Loading preview…
search-preview-failed = Preview unavailable: { $error }
search-error = Search failed: { $error }
search-footer = ↑↓ select · Enter open · { $split } open to the side · Esc close

## Find in Files
search-fif-title-find = Find in Files
search-fif-title-replace = Replace in Files
search-fif-query = Search text
search-fif-replacement = Replace with
search-fif-filter = Files to include (e.g. src/ or .rs)
search-fif-case = Match case (Alt+C)
search-fif-word = Match whole word (Alt+W)
search-fif-regex = Use regular expression (Alt+R)
search-fif-toggle-replace = Toggle replace
search-fif-replace-all = Replace All
search-fif-hint = Type to search across the project
search-fif-empty = No results
search-fif-summary = Results: { $matches } in files: { $files }
search-fif-truncated = Only the first { $limit } lines are shown
search-fif-open-hint = Enter or double-click opens the match
search-fif-replaced = Replaced { $count } occurrences in { $files } open files.
search-fif-skipped = Files not open in an editor were skipped ({ $skipped }): writing files arrives with the real project service (Stage 4).
search-fif-nothing-replaced = Nothing replaced: open a file containing matches first.
