//! Comment syntax per language, used by Toggle Line / Block Comment.
//!
//! Until Language Packs arrive (Stage 5) the table lives here; a pack's
//! manifest will provide the same two fields.

/// A language's comment delimiters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CommentTokens {
    pub line: Option<&'static str>,
    pub block: Option<(&'static str, &'static str)>,
}

impl CommentTokens {
    pub const NONE: CommentTokens = CommentTokens {
        line: None,
        block: None,
    };

    const C_LIKE: CommentTokens = CommentTokens {
        line: Some("//"),
        block: Some(("/*", "*/")),
    };
    const HASH: CommentTokens = CommentTokens {
        line: Some("#"),
        block: None,
    };
    const MARKUP: CommentTokens = CommentTokens {
        line: None,
        block: Some(("<!--", "-->")),
    };

    /// Comment tokens for a language id (`"rust"`, `"python"`, …).
    pub fn for_language(language: &str) -> Self {
        match language {
            "rust" | "c" | "cpp" | "csharp" | "java" | "javascript" | "typescript" | "tsx"
            | "go" | "kotlin" | "swift" | "scala" | "dart" | "php" | "zig" | "groovy" | "jsonc"
            | "proto" => Self::C_LIKE,
            "python" | "toml" | "yaml" | "bash" | "shell" | "sh" | "ruby" | "perl" | "r"
            | "make" | "makefile" | "dockerfile" | "cmake" | "elixir" | "nix" | "ini"
            | "properties" | "gitignore" => Self::HASH,
            "html" | "xml" | "svg" | "markdown" | "vue" => Self::MARKUP,
            "css" | "scss" | "less" => CommentTokens {
                line: if language == "css" { None } else { Some("//") },
                block: Some(("/*", "*/")),
            },
            "sql" | "lua" | "haskell" => CommentTokens {
                line: Some("--"),
                block: Some(match language {
                    "lua" => ("--[[", "]]"),
                    "haskell" => ("{-", "-}"),
                    _ => ("/*", "*/"),
                }),
            },
            "lisp" | "clojure" | "scheme" | "asm" => CommentTokens {
                line: Some(";"),
                block: None,
            },
            "erlang" | "latex" | "tex" | "matlab" => CommentTokens {
                line: Some("%"),
                block: None,
            },
            "bat" | "batch" => CommentTokens {
                line: Some("REM"),
                block: None,
            },
            "vim" => CommentTokens {
                line: Some("\""),
                block: None,
            },
            _ => Self::NONE,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_languages() {
        assert_eq!(CommentTokens::for_language("rust").line, Some("//"));
        assert_eq!(CommentTokens::for_language("python").line, Some("#"));
        assert_eq!(
            CommentTokens::for_language("markdown").block,
            Some(("<!--", "-->"))
        );
        assert_eq!(CommentTokens::for_language("json"), CommentTokens::NONE);
    }
}
