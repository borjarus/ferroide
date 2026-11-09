## FerroIDE — Tool windows: Project, Problems, Terminal, Run, Debug, Structure, TODO, Notifications, Services.
## Every message here must also exist in pl/panels.ftl (checked by tests).

## Shared
panels-loading = Loading…
panels-load-failed = Could not load
panels-refresh = Refresh
panels-time-now = just now
panels-time-minutes = { $n } min ago
panels-time-hours = { $n } h ago
panels-time-days = { $n ->
    [one] { $n } day ago
   *[other] { $n } days ago
}

## Project
panels-project-open = Open
panels-project-new-directory = New Directory…
panels-project-new-directory-in = New directory in { $dir }
panels-project-rename = Rename…
panels-project-delete = Delete
panels-project-delete-title = Delete?
panels-project-delete-body = Delete “{ $name }”? This cannot be undone.
panels-project-copy-path = Copy Path
panels-project-copy-relative-path = Copy Relative Path
panels-project-name-placeholder = Name (Enter to confirm, Esc to cancel)
panels-project-matches = { $count ->
    [one] { $count } match
   *[other] { $count } matches
}

## Problems
panels-problems-errors = Errors: { $count }
panels-problems-warnings = Warnings: { $count }
panels-problems-infos = Info: { $count }
panels-problems-sort-severity = Sort by severity
panels-problems-count = { $count ->
    [one] { $count } problem
   *[other] { $count } problems
}
panels-problems-empty = No problems found

## Terminal
panels-terminal-close = Close session
panels-terminal-empty = No terminal sessions. Press + to open one.
panels-terminal-readonly = Read-only preview: the interactive terminal arrives in Stage 8.

## Run
panels-run-console = Console
panels-run-tests = Tests
panels-run-hide-passed = Hide passed tests
panels-run-no-tests = No test results yet
panels-run-select-test = Select a test to see its output.
panels-run-summary = { $passed } passed, { $failed } failed, { $ignored } ignored

## Debug
panels-debug-resume = Resume
panels-debug-pause = Pause
panels-debug-step-over = Step Over
panels-debug-step-into = Step Into
panels-debug-step-out = Step Out
panels-debug-stop = Stop
panels-debug-frames = Frames
panels-debug-console = Console
panels-debug-breakpoints = Breakpoints
panels-debug-paused = paused
panels-debug-running = running
panels-debug-library-frames = { $count ->
    [one] Show { $count } library frame
   *[other] Show { $count } library frames
}
panels-debug-no-variables = No variables in this frame
panels-debug-no-breakpoints = No breakpoints
panels-debug-remove-breakpoint = Remove breakpoint
panels-debug-evaluate = Evaluate expression (Enter)
panels-debug-mock-step = { $step }: not available on mock services (Stage 10).

## Structure
panels-structure-filter = Filter symbols
panels-structure-no-file = Open a file to see its structure
panels-structure-empty = No symbols in this file

## TODO
panels-todo-summary = { $todo } TODO, { $fixme } FIXME
panels-todo-count = { $count }
panels-todo-empty = No TODO or FIXME comments

## Notifications
panels-notifications-mark-read = Mark all as read
panels-notifications-clear = Clear all
panels-notifications-empty = No notifications

## Services
panels-services-running = running
panels-services-stopped = stopped
panels-services-failed = failed
panels-services-empty = No services
