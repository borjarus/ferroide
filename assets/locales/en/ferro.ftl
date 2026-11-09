## FerroIDE — English UI strings.
## Every message here must also exist in pl/ferro.ftl (checked by tests).

## Menus
menu-file = File
menu-view = View
menu-window = Window
menu-run = Run
menu-help = Help
menu-tool-windows = Tool Windows
menu-appearance = Appearance
menu-language = Language
menu-keymap = Keymap
menu-navigate = Navigate
menu-code = Code
menu-vcs = Git

## Action categories (Find Action)
category-file = File
category-view = View
category-navigate = Navigate
category-code = Code
category-vcs = Version Control
category-tool-windows = Tool Windows
category-editor = Editor
category-run = Run
category-appearance = Appearance
category-help = Help

## Actions
action-new-project = New Project…
action-open-project = Open…
action-get-from-vcs = Get from VCS…
action-close-project = Close Project
action-new-window = New Window
action-open-settings = Settings…
action-quit = Exit
action-find-action = Find Action…
action-search-everywhere = Search Everywhere
action-hide-all-tool-windows = Hide All Tool Windows
action-maximize-tool-window = Maximize Tool Window
action-reset-layout = Restore Default Layout
action-toggle-compact-mode = Compact Mode
action-zoom-in-ui = Zoom In IDE
action-zoom-out-ui = Zoom Out IDE
action-reset-ui-zoom = Reset IDE Zoom
action-toggle-project = Project
action-toggle-commit = Commit
action-toggle-structure = Structure
action-toggle-run = Run
action-toggle-debug = Debug
action-toggle-problems = Problems
action-toggle-git = Git
action-toggle-terminal = Terminal
action-toggle-todo = TODO
action-toggle-notifications = Notifications
action-toggle-database = Database
action-toggle-build = Build
action-close-tab = Close Tab
action-close-other-tabs = Close Other Tabs
action-close-all-tabs = Close All Tabs
action-pin-tab = Pin / Unpin Tab
action-next-tab = Select Next Tab
action-previous-tab = Select Previous Tab
action-split-right = Split Right
action-split-down = Split Down
action-run = Run
action-debug = Debug
action-stop = Stop
action-theme-dark = Ferro Dark
action-theme-light = Ferro Light
action-theme-high-contrast = High Contrast
action-language-english = Language: English
action-language-polish = Language: Polish
action-keymap-jetbrains = Keymap: JetBrains (IntelliJ IDEA)
action-keymap-zed = Keymap: Zed
action-keymap-sublime = Keymap: Sublime Text
action-keymap-emacs = Keymap: Emacs
action-toggle-vim-mode = Toggle Vim Mode
action-new-file = New File…
action-open-settings-file = Open Settings File (settings.toml)
action-open-keymap-settings = Keyboard Shortcuts
action-toggle-left-dock = Toggle Left Dock
action-toggle-bottom-dock = Toggle Bottom Dock
action-toggle-right-dock = Toggle Right Dock
action-quick-open = Go to File…
action-command-palette = Show All Commands
action-go-to-symbol-in-file = Go to Symbol in File…
action-go-to-symbol-in-workspace = Go to Symbol in Workspace…
action-go-back = Back
action-go-forward = Forward
action-go-to-line = Go to Line…
action-find-in-files = Find in Files…
action-replace-in-files = Replace in Files…
action-select-opened-file = Select Opened File
action-next-problem = Next Problem
action-previous-problem = Previous Problem
action-trigger-completion = Trigger Completion
action-quick-fix = Quick Fix…
action-quick-documentation = Quick Documentation
action-parameter-info = Parameter Info
action-rename-symbol = Rename Symbol…
action-toggle-breakpoint = Toggle Breakpoint
action-new-terminal = New Terminal
action-toggle-services = Services
action-reopen-closed-tab = Reopen Closed Tab
action-edit-run-configurations = Edit Run Configurations…
action-show-diff = Show Diff
action-resolve-conflicts = Resolve Conflicts…
action-keymap-vscode = Keymap: VS Code
action-show-welcome = Welcome Screen
action-about = About FerroIDE

## Tool windows
tool-project = Project
tool-commit = Commit
tool-structure = Structure
tool-run = Run
tool-debug = Debug
tool-problems = Problems
tool-git = Git
tool-terminal = Terminal
tool-todo = TODO
tool-notifications = Notifications
tool-database = Database
tool-build = Build
tool-empty = Nothing to show yet
tree-loading = Loading…
tree-load-failed = Could not load
run-empty = Nothing has run yet. Press Run in the toolbar.
run-finished = Process finished with exit code { $code }
debug-empty = No active debug session
database-empty = No data sources configured
commit-changes = Changes
commit-message = Commit message
commit-button = Commit

## Main toolbar
toolbar-project = Project
toolbar-branch = Git branch
toolbar-run-config = Run configuration
toolbar-settings = Settings
toolbar-main-menu = Main menu
dock-move-left = Dock Left
dock-move-bottom = Dock Bottom
dock-move-right = Dock Right
run-no-config = Add Configuration…
vcs-no-branch = No VCS
vcs-loading = Loading branches…
vcs-local-branches = Local
vcs-remote-branches = Remote

## Editor
editor-loading = Opening file…
editor-load-failed = The file could not be opened
tab-pinned-cannot-close = Pinned tabs are not closed. Unpin it first.

## Status bar
status-cursor = Caret position (line:column)
status-line-separator = Line separator
status-encoding = File encoding
status-indent = Indentation
status-indent-4 = 4 spaces
status-language = File language
status-memory = { $mb } MB
status-memory-tooltip = Memory used by FerroIDE (excluding language servers)
lsp-ready = OK
lsp-starting = starting
lsp-crashed = crashed
task-indexing = Indexing…

## Find Action
find-action-placeholder = Type an action name…

## Settings
settings-title = Settings
settings-theme = Theme
settings-language = Language
settings-keymap = Keymap
settings-ui-size = IDE font size
settings-layout = Layout
settings-stage-note = The full Settings window arrives in Stage 2.

## About
about-version = Version { $version }
about-stage = Stage 2 preview (M1 "UI Preview"): every main IDE view, running on mock services.
about-theme-switch = Last theme switch: { $took }

## Welcome screen
welcome-title = Welcome to FerroIDE
welcome-projects = Projects
welcome-recent = Recent projects
welcome-search = Search projects
welcome-no-projects = No recent projects
welcome-today = today
welcome-yesterday = yesterday
welcome-days-ago = { $days ->
    [one] { $days } day ago
   *[other] { $days } days ago
}

## Stage 2 shared
feature-coming-soon = This view is not available yet.
tool-services = Services
