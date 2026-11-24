# Language Packs

Every language FerroIDE knows is a pack: a folder with a `language.toml`
manifest and tree-sitter queries (`.scm`). The packs in this folder are
built in — `crates/ferro_languages/build.rs` embeds every `language.toml`
and `.scm` file at build time. Packs in `<config dir>/ferroide/languages/`
are loaded at startup and replace a built-in pack with the same `id`.

## Manifest

```toml
id = "python"                      # used by injections, settings, modelines
name = "Python"                    # shown in the status bar
aliases = ["py"]                   # Markdown fences, modelines, injections
file_extensions = ["py", "pyi"]    # longest match wins: "d.ts" before "ts"
file_names = ["SConstruct"]        # exact names
shebangs = ["python", "python3"]   # "python3.12" matches both
first_line = '^<\?xml'             # optional regex on the first line
project_markers = ["pyproject.toml"]
hidden = false                     # helper grammars (markdown_inline)
syntax_errors = true               # parser errors as diagnostics

line_comment = "#"
block_comment = ['"""', '"""']
indent = { unit = "    ", tab_size = 4 }
brackets = [["(", ")"], ["[", "]"], ["{", "}"]]          # default
auto_pairs = [["(", ")"], ["\"", "\""]]                   # default: brackets + quotes
increase_indent_pattern = '^.*:\s*$'                      # Enter after a match indents
decrease_indent_pattern = '^\s*(elif|else)\b'             # a matching line outdents

[grammar]
name = "python"                    # a built-in grammar (crates/ferro_languages/src/grammars.rs)
queries = ["highlights.scm", "tags.scm", "folds.scm"]

[[language_servers]]               # Stage 6
id = "basedpyright"
command = "basedpyright-langserver"
args = ["--stdio"]
install = { pypi = "basedpyright" }
```

The full example is in the plan, Appendix B. `[[language_servers]]`,
`[debugger]`, `[[run_templates]]` and `[[test_frameworks]]` are parsed and
kept for Stages 6, 8 and 10.

Detection order: a Vim or Emacs modeline in the first or last five lines,
then the file name, the extension, the shebang and `first_line`.

## Queries

A query listed under `[grammar] queries` is read from the pack's folder; if
the folder has no such file, the grammar's own query is used (most grammar
crates ship `highlights.scm` and `tags.scm`). A pack file whose first line
is `; inherits: grammar` is added to the grammar's query instead of
replacing it.

| File | Captures | Used for |
|---|---|---|
| `highlights.scm` | highlight names: `keyword`, `function.method`, `string.escape`… (`a.b` falls back to `a`) | highlighting |
| `injections.scm` | `@injection.content`, `@injection.language` or `#set! injection.language "…"`, `#set! injection.combined` | JS/CSS in HTML, code blocks in Markdown, SQL in strings |
| `locals.scm` | `@local.scope`, `@local.definition`, `@local.reference` | highlighting of local variables |
| `tags.scm` | `@definition.<kind>` / `@reference.<kind>` with `@name` | the project symbol index (Go to Symbol) |
| `outline.scm` | as `tags.scm` | the file structure; falls back to `tags.scm` |
| `folds.scm` | `@fold` | folding; without it, multi-line bracketed nodes fold |
| `indents.scm` | `@indent`, `@outdent` | Auto-Indent Lines; without it, bracket counting |
| `runnables.scm` | `@run`, `@name`, `#set! tag "main" \| "test" \| "test-module"` | ▶ icons in the gutter |
| `brackets.scm` | `@open`, `@close` | bracket matching where brackets are not plain tokens |

Indentation: a line is one level deeper than the line where the innermost
`@indent` node containing it starts (if that node started on an earlier
line), and one level less when the line starts with an `@outdent` token.

Regular expressions in `#match?` are Rust `regex` syntax; inside a query
string a backslash is written twice: `"^\\s*select"`.

Every built-in pack is compiled by `cargo test -p ferro_languages`, and by
GPUI Kit's highlighter in `ferro_workspace`'s `tests_syntax.rs`.

## Licenses

The grammars and the queries they ship come from the `tree-sitter-*`
crates (MIT); they are linked, not copied. The `.scm` files in this folder
are FerroIDE's own (Apache-2.0 OR MIT).
