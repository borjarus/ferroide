## FerroIDE — Dialogs: Rename, New File, Run Configurations, Clone Repository.
## Every message here must also exist in pl/dialogs.ftl (checked by tests).

## New File
dialogs-new-file-name = Name
dialogs-new-file-name-placeholder = e.g. utils.rs
dialogs-new-file-directory = Directory (relative to the project)
dialogs-new-file-directory-placeholder = project root
dialogs-new-file-kind-file = File
dialogs-new-file-kind-directory = Directory
dialogs-new-file-create = Create
dialogs-new-file-error-empty = Enter a name.
dialogs-new-file-error-invalid = The name contains characters that are not allowed, or begins or ends with a space or dot.
dialogs-new-file-error-outside = The directory must be inside the project.

## Run Configurations
dialogs-run-title = Run Configurations
dialogs-run-name = Name
dialogs-run-kind = Type
dialogs-run-command = Command
dialogs-run-arguments = Program arguments
dialogs-run-working-directory = Working directory
dialogs-run-environment = Environment variables
dialogs-run-env-add = Add variable
dialogs-run-env-value = value
dialogs-run-add = Add configuration
dialogs-run-duplicate = Duplicate
dialogs-run-remove = Remove
dialogs-run-empty = No run configurations
dialogs-run-unnamed = Unnamed
dialogs-run-copy = copy
dialogs-run-error-name = Every configuration needs a name.
dialogs-run-error-command = Every configuration needs a command.

## Clone Repository
dialogs-clone-title = Clone Repository
dialogs-clone-url = Repository URL
dialogs-clone-directory = Directory
dialogs-clone-button = Clone
dialogs-clone-stop = Stop
dialogs-clone-progress = Cloning…
dialogs-clone-error-url-empty = Enter the repository URL.
dialogs-clone-error-url-invalid = This does not look like a Git URL (https://…, ssh://…, git@host:path or a path ending in .git).
dialogs-clone-error-directory = Choose a directory to clone into.

## Rename
dialogs-rename-symbol = Rename “{ $name }” and its usages to:
dialogs-rename-new-name = New name
dialogs-rename-apply = Rename
dialogs-rename-type-name = Type a new name to see the changes.
dialogs-rename-loading = Finding usages…
dialogs-rename-no-usages = No usages found.
dialogs-rename-summary = { $count ->
    [one] 1 change
   *[other] { $count } changes
} in { $files ->
    [one] 1 file
   *[other] { $files } files
}
dialogs-rename-done = Renamed { $count ->
    [one] 1 occurrence
   *[other] { $count } occurrences
} in { $files ->
    [one] 1 file
   *[other] { $files } files
}.
dialogs-rename-skipped = { $files ->
    [one] 1 file is not open and was left unchanged.
   *[other] { $files } files are not open and were left unchanged.
}
dialogs-rename-no-symbol = Place the caret on a symbol to rename it.
dialogs-rename-error-empty = Enter a name.
dialogs-rename-error-invalid = Not a valid identifier.
dialogs-rename-error-keyword = A keyword cannot be used as a name.
