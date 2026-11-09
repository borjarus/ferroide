## FerroIDE — Version control: Commit, Git log, diff viewer and merge.
## Every message here must also exist in pl/vcs.ftl (checked by tests).

## Common
vcs-refresh = Refresh
vcs-retry = Retry
vcs-show-diff = Show Diff
vcs-file-count = { $count ->
    [one] { $count } file
   *[other] { $count } files
}
vcs-time-now = just now
vcs-time-minutes = { $n } min ago
vcs-time-hours = { $n } h ago
vcs-time-days = { $n ->
    [one] { $n } day ago
   *[other] { $n } days ago
}
vcs-kind-modified = modified
vcs-kind-added = added
vcs-kind-deleted = deleted
vcs-kind-renamed = renamed
vcs-kind-untracked = unversioned
vcs-kind-conflicted = merge conflict

## Commit tool window
vcs-changes = Changes
vcs-loading-changes = Reading changes…
vcs-changes-failed = Could not read the changes
vcs-no-changes = No changes
vcs-rollback = Rollback
vcs-rollback-not-available = Rollback is not available on mock data.
vcs-expand-all = Expand All
vcs-collapse-all = Collapse All
vcs-amend = Amend
vcs-amend-not-available = Amend is not available on mock data; a new commit is created.
vcs-selected-count = { $count ->
    [one] { $count } file selected
   *[other] { $count } files selected
}
vcs-commit-message = Commit message
vcs-commit-message-placeholder = Commit message
vcs-commit = Commit
vcs-committed = Committed { $count ->
    [one] { $count } file
   *[other] { $count } files
} as { $hash }

## Git log
vcs-log = Git log
vcs-log-filter-placeholder = Filter by text, author or hash
vcs-log-branch-filter = Filter by branch
vcs-log-all-branches = All branches
vcs-log-loading = Reading the log…
vcs-log-failed = Could not read the log
vcs-log-empty = No commits yet
vcs-log-no-match = No commits match the filter
vcs-log-details = Commit details
vcs-log-authored = { $author }, { $date }
vcs-log-merge-of = Merge of { $count } parents

## Diff viewer
vcs-diff = Diff
vcs-diff-tab = { $file } (diff)
vcs-diff-loading = Computing the diff…
vcs-diff-failed = Could not compute the diff
vcs-diff-no-changes = The file has no changes
vcs-diff-side-by-side = Side by side
vcs-diff-unified = Unified
vcs-diff-next-change = Next change (F7)
vcs-diff-previous-change = Previous change (Shift+F7)

## Merge
vcs-merge = Merge
vcs-merge-tab = { $file } (merge)
vcs-merge-loading = Reading the conflicts…
vcs-merge-failed = Could not read the conflicts
vcs-merge-no-conflicts = There are no merge conflicts.
vcs-merge-no-conflicts-in-file = This file has no merge conflicts
vcs-merge-ours = Yours ({ $label })
vcs-merge-theirs = Theirs ({ $label })
vcs-merge-base = Base
vcs-merge-result = Result
vcs-merge-unresolved = Unresolved conflict
vcs-merge-accept-left = Accept Left
vcs-merge-accept-right = Accept Right
vcs-merge-accept-both = Accept Both
vcs-merge-ignore = Ignore
vcs-merge-next-conflict = Next conflict (F7)
vcs-merge-previous-conflict = Previous conflict (Shift+F7)
vcs-merge-counter = Conflict { $current } of { $total } · { $unresolved } unresolved
vcs-merge-show-base = Show Base
vcs-merge-apply = Apply
vcs-merge-done = Resolved
vcs-merge-applied = { $file }: conflicts resolved
