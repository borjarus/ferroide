## FerroIDE — Settings window and keymap page.
## Every message here must also exist in pl/settings.ftl (checked by tests).

## Settings window
settings-page-appearance = Appearance & Behavior
settings-page-editor = Editor
settings-page-keymap = Keymap
settings-search-placeholder = Search settings
settings-no-results = No settings match the search
settings-jump-to-page = Go to this page
settings-unsaved = Unsaved changes
settings-reset = Reset
settings-cancel = Cancel
settings-apply = Apply
settings-ok = OK
settings-reset-to-default = Reset to default
settings-decrease = Decrease
settings-increase = Increase

settings-field-theme = Theme
settings-field-theme-desc = Colors of the whole IDE; switching takes effect immediately.
settings-field-ui-font-size = IDE font size
settings-field-ui-font-size-desc = Size of menus, tool windows and dialogs, in pixels.
settings-field-compact = Compact mode
settings-field-compact-desc = A slimmer toolbar with the main menu behind a button.
settings-field-animations = Animations
settings-field-animations-desc = Turn off on slow machines or remote desktops.
settings-field-language = Language
settings-field-language-desc = Language of the user interface.
settings-field-editor-font-size = Editor font size
settings-field-editor-font-size-desc = Size of the code font, in pixels.
settings-field-tab-size = Tab size
settings-field-tab-size-desc = Spaces per indentation level.
settings-field-tab-limit = Tab limit
settings-field-tab-limit-desc = Above this many open tabs the least recently used unpinned tab closes.
settings-field-soft-wrap = Soft wrap
settings-field-soft-wrap-desc = Wrap long lines at the window edge.
settings-field-line-numbers = Line numbers
settings-field-line-numbers-desc = Show line numbers in the gutter.
settings-field-inlay-hints = Inlay hints
settings-field-inlay-hints-desc = Show inferred types and parameter names inline.
settings-field-sticky-lines = Sticky lines
settings-field-sticky-lines-desc = Keep the enclosing scopes at the top while scrolling.
settings-field-breadcrumbs = Breadcrumbs
settings-field-breadcrumbs-desc = Show the path to the current symbol above the editor.
settings-field-vcs-markers = Change markers
settings-field-vcs-markers-desc = Mark added, modified and deleted lines in the gutter.
settings-field-preview-tabs = Preview tabs
settings-field-preview-tabs-desc = A single click opens a temporary tab that the next one replaces.
settings-field-keymap-scheme = Keymap scheme
settings-field-keymap-scheme-desc = The set of shortcuts your own bindings are applied on top of.
settings-field-vim-mode = Vim mode
settings-field-vim-mode-desc = Modal editing on top of any scheme.

## Welcome screen onboarding
settings-welcome-get-started = Get started
settings-welcome-choose-theme = Choose theme
settings-welcome-choose-keymap = Choose keymap

## Keymap page
keymap-scheme = Scheme
keymap-search-placeholder = Search by action name or shortcut
keymap-search-by-keys = Search by pressed keys
keymap-press-keys = Press keys…
keymap-conflicts = Conflicts ({ $count })
keymap-reset-all = Reset all overrides
keymap-no-results = No actions match the search
keymap-unbound = —
keymap-user = User
keymap-edit = Edit shortcut
keymap-edit-for = Shortcut for “{ $action }”
keymap-remove = Remove shortcut
keymap-reset = Reset to scheme default
keymap-vim-note = Stored now; modal editing arrives with the editor core.
keymap-record-hint = Press the new shortcut; a second key makes a chord. Enter saves, Escape cancels.
keymap-conflict-with = Also bound to: { $actions }
keymap-clear = Clear
keymap-save = Save
