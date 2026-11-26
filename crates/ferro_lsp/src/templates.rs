//! Live Templates and Postfix Completion (IntelliJ IDEA): built-in snippets
//! per language, offered by completion next to the server's items.
//!
//! - A live template expands an abbreviation: `fori` → a `for` loop.
//! - A postfix template wraps the expression before the dot: `x.if` →
//!   `if x { }`, `list.for` → a loop over `list`.
//!
//! Bodies use the LSP snippet format; `\t` is one indentation level and
//! `$EXPR` the wrapped expression.

use ferro_services::{CompletionItem, SymbolKind, TextPos, TextRange};

use crate::snippet;

pub struct Template {
    pub abbreviation: &'static str,
    pub description: &'static str,
    pub body: &'static str,
}

const fn t(abbreviation: &'static str, description: &'static str, body: &'static str) -> Template {
    Template {
        abbreviation,
        description,
        body,
    }
}

const RUST: &[Template] = &[
    t("fn", "function", "fn ${1:name}($2) {\n\t$0\n}"),
    t(
        "test",
        "test function",
        "#[test]\nfn ${1:name}() {\n\t$0\n}",
    ),
    t(
        "tmod",
        "test module",
        "#[cfg(test)]\nmod tests {\n\tuse super::*;\n\n\t#[test]\n\tfn ${1:name}() {\n\t\t$0\n\t}\n}",
    ),
    t("main", "main function", "fn main() {\n\t$0\n}"),
    t("impl", "impl block", "impl ${1:Type} {\n\t$0\n}"),
    t("st", "struct", "struct ${1:Name} {\n\t$0\n}"),
    t("en", "enum", "enum ${1:Name} {\n\t$0\n}"),
    t("for", "for loop", "for ${1:item} in ${2:iter} {\n\t$0\n}"),
    t("println", "println!", "println!(\"$1\");"),
    t(
        "derive",
        "derive attribute",
        "#[derive(Debug, Clone, PartialEq)]",
    ),
];

const RUST_POSTFIX: &[Template] = &[
    t("if", "if expr { }", "if $EXPR {\n\t$0\n}"),
    t("not", "!expr", "!$EXPR"),
    t("match", "match expr { }", "match $EXPR {\n\t$0\n}"),
    t(
        "for",
        "for item in expr { }",
        "for ${1:item} in $EXPR {\n\t$0\n}",
    ),
    t("let", "let name = expr;", "let ${1:name} = $EXPR;"),
    t("dbg", "dbg!(expr)", "dbg!($EXPR)"),
    t("ok", "Ok(expr)", "Ok($EXPR)"),
    t("some", "Some(expr)", "Some($EXPR)"),
    t("ref", "&expr", "&$EXPR"),
    t(
        "println",
        "println!(\"{}\", expr)",
        "println!(\"{}\", $EXPR);",
    ),
];

const PYTHON: &[Template] = &[
    t("def", "function", "def ${1:name}($2):\n\t${0:pass}"),
    t(
        "class",
        "class",
        "class ${1:Name}:\n\tdef __init__(self$2):\n\t\t${0:pass}",
    ),
    t(
        "main",
        "if __name__ == \"__main__\"",
        "if __name__ == \"__main__\":\n\t${0:main()}",
    ),
    t(
        "for",
        "for loop",
        "for ${1:item} in ${2:items}:\n\t${0:pass}",
    ),
    t(
        "test",
        "pytest function",
        "def test_${1:name}():\n\t${0:assert True}",
    ),
    t(
        "with",
        "with statement",
        "with ${1:open(path)} as ${2:f}:\n\t${0:pass}",
    ),
];

const PYTHON_POSTFIX: &[Template] = &[
    t("if", "if expr:", "if $EXPR:\n\t${0:pass}"),
    t("not", "not expr", "not $EXPR"),
    t(
        "for",
        "for item in expr:",
        "for ${1:item} in $EXPR:\n\t${0:pass}",
    ),
    t("print", "print(expr)", "print($EXPR)"),
    t("len", "len(expr)", "len($EXPR)"),
    t("return", "return expr", "return $EXPR"),
];

const JS: &[Template] = &[
    t("fn", "function", "function ${1:name}($2) {\n\t$0\n}"),
    t(
        "af",
        "arrow function",
        "const ${1:name} = ($2) => {\n\t$0\n};",
    ),
    t("log", "console.log", "console.log($1);"),
    t(
        "fori",
        "indexed for loop",
        "for (let ${1:i} = 0; $1 < ${2:n}; $1++) {\n\t$0\n}",
    ),
    t(
        "forof",
        "for…of loop",
        "for (const ${1:item} of ${2:items}) {\n\t$0\n}",
    ),
    t("test", "test()", "test(\"${1:name}\", () => {\n\t$0\n});"),
    t(
        "desc",
        "describe()",
        "describe(\"${1:name}\", () => {\n\t$0\n});",
    ),
];

const JS_POSTFIX: &[Template] = &[
    t("if", "if (expr) { }", "if ($EXPR) {\n\t$0\n}"),
    t("not", "!expr", "!$EXPR"),
    t("log", "console.log(expr)", "console.log($EXPR);"),
    t("const", "const name = expr;", "const ${1:name} = $EXPR;"),
    t("return", "return expr;", "return $EXPR;"),
    t("await", "await expr", "await $EXPR"),
    t(
        "forof",
        "for (const item of expr)",
        "for (const ${1:item} of $EXPR) {\n\t$0\n}",
    ),
];

const GO: &[Template] = &[
    t("fn", "function", "func ${1:name}($2) {\n\t$0\n}"),
    t("main", "main function", "func main() {\n\t$0\n}"),
    t(
        "test",
        "test function",
        "func Test${1:Name}(t *testing.T) {\n\t$0\n}",
    ),
    t(
        "iferr",
        "if err != nil",
        "if err != nil {\n\treturn ${1:err}\n}",
    ),
    t(
        "forr",
        "for range",
        "for ${1:_}, ${2:v} := range ${3:items} {\n\t$0\n}",
    ),
];

const GO_POSTFIX: &[Template] = &[
    t("if", "if expr { }", "if $EXPR {\n\t$0\n}"),
    t("not", "!expr", "!$EXPR"),
    t(
        "forr",
        "for range expr",
        "for ${1:_}, ${2:v} := range $EXPR {\n\t$0\n}",
    ),
    t("print", "fmt.Println(expr)", "fmt.Println($EXPR)"),
    t("return", "return expr", "return $EXPR"),
];

const JAVA: &[Template] = &[
    t(
        "psvm",
        "main method",
        "public static void main(String[] args) {\n\t$0\n}",
    ),
    t("sout", "System.out.println", "System.out.println($1);"),
    t(
        "fori",
        "indexed for loop",
        "for (int ${1:i} = 0; $1 < ${2:n}; $1++) {\n\t$0\n}",
    ),
    t(
        "iter",
        "for-each loop",
        "for (${1:var} ${2:item} : ${3:items}) {\n\t$0\n}",
    ),
    t("test", "JUnit test", "@Test\nvoid ${1:name}() {\n\t$0\n}"),
];

const JAVA_POSTFIX: &[Template] = &[
    t("if", "if (expr) { }", "if ($EXPR) {\n\t$0\n}"),
    t("not", "!expr", "!$EXPR"),
    t(
        "sout",
        "System.out.println(expr)",
        "System.out.println($EXPR);",
    ),
    t("var", "var name = expr;", "var ${1:name} = $EXPR;"),
    t(
        "for",
        "for (var item : expr)",
        "for (var ${1:item} : $EXPR) {\n\t$0\n}",
    ),
    t("return", "return expr;", "return $EXPR;"),
];

const C_LIKE: &[Template] = &[
    t(
        "main",
        "main function",
        "int main(int argc, char *argv[]) {\n\t$0\n\treturn 0;\n}",
    ),
    t(
        "fori",
        "indexed for loop",
        "for (int ${1:i} = 0; $1 < ${2:n}; $1++) {\n\t$0\n}",
    ),
    t("inc", "#include", "#include <${1:stdio.h}>"),
];

const C_LIKE_POSTFIX: &[Template] = &[
    t("if", "if (expr) { }", "if ($EXPR) {\n\t$0\n}"),
    t("not", "!expr", "!$EXPR"),
    t("return", "return expr;", "return $EXPR;"),
];

const CSHARP: &[Template] = &[
    t("cw", "Console.WriteLine", "Console.WriteLine($1);"),
    t(
        "prop",
        "property",
        "public ${1:int} ${2:Name} { get; set; }",
    ),
    t(
        "fori",
        "indexed for loop",
        "for (int ${1:i} = 0; $1 < ${2:n}; $1++)\n{\n\t$0\n}",
    ),
    t(
        "foreach",
        "foreach loop",
        "foreach (var ${1:item} in ${2:items})\n{\n\t$0\n}",
    ),
];

/// The live templates of a language (by Language Pack id).
pub fn live_templates(language: &str) -> &'static [Template] {
    match language {
        "rust" => RUST,
        "python" => PYTHON,
        "javascript" | "typescript" | "tsx" => JS,
        "go" => GO,
        "java" => JAVA,
        "c" | "cpp" => C_LIKE,
        "csharp" => CSHARP,
        _ => &[],
    }
}

/// The postfix templates of a language.
pub fn postfix_templates(language: &str) -> &'static [Template] {
    match language {
        "rust" => RUST_POSTFIX,
        "python" => PYTHON_POSTFIX,
        "javascript" | "typescript" | "tsx" => JS_POSTFIX,
        "go" => GO_POSTFIX,
        "java" => JAVA_POSTFIX,
        "c" | "cpp" | "csharp" => C_LIKE_POSTFIX,
        _ => &[],
    }
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// A snippet body as text: `\t` replaced by `indent_unit`, every line after
/// the first indented like the line the caret is on.
pub fn expand(body: &str, line_indent: &str, indent_unit: &str) -> String {
    let text = snippet::to_text(body);
    let mut out = String::with_capacity(text.len());
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
            out.push_str(line_indent);
        }
        let tabs = line.chars().take_while(|c| *c == '\t').count();
        for _ in 0..tabs {
            out.push_str(indent_unit);
        }
        out.push_str(&line[tabs..]);
    }
    out
}

/// The start (char column) of the expression ending at `end` in `line`: an
/// identifier chain with `.`, `::`, calls and indexing.
fn expression_start(line: &[char], end: usize) -> Option<usize> {
    let mut i = end;
    let mut depth = 0i32;
    while i > 0 {
        let c = line[i - 1];
        match c {
            ')' | ']' => depth += 1,
            '(' | '[' if depth > 0 => depth -= 1,
            '(' | '[' => break,
            _ if depth > 0 => {}
            c if is_word(c) || c == '.' || c == ':' || c == '"' || c == '\'' => {}
            _ => break,
        }
        i -= 1;
    }
    let start = line[i..end]
        .iter()
        .position(|c| !matches!(c, '.' | ':'))
        .map(|p| i + p)?;
    (start < end && depth == 0).then_some(start)
}

/// Live and postfix templates for the word before the caret.
pub fn completions(
    language: &str,
    text: &str,
    pos: TextPos,
    indent_unit: &str,
) -> Vec<CompletionItem> {
    let Some(line) = text.split('\n').nth(pos.line as usize) else {
        return Vec::new();
    };
    let line = line.strip_suffix('\r').unwrap_or(line);
    let chars: Vec<char> = line.chars().collect();
    let caret = (pos.column as usize).min(chars.len());
    let word_start = chars[..caret]
        .iter()
        .rposition(|c| !is_word(*c))
        .map_or(0, |i| i + 1);
    let word: String = chars[word_start..caret].iter().collect();
    let indent: String = chars.iter().take_while(|c| c.is_whitespace()).collect();
    let mut items = Vec::new();

    if word_start > 0 && chars[word_start - 1] == '.' {
        if let Some(start) = expression_start(&chars, word_start - 1) {
            let expr: String = chars[start..word_start - 1].iter().collect();
            for template in postfix_templates(language) {
                if !template.abbreviation.starts_with(&word) {
                    continue;
                }
                let body = template.body.replace("$EXPR", &expr.replace('$', "\\$"));
                items.push(CompletionItem {
                    label: format!(".{}", template.abbreviation),
                    kind: SymbolKind::Snippet,
                    detail: Some(template.description.to_string()),
                    documentation: Some(format!(
                        "Postfix template\n\n```\n{}\n```",
                        expand(&body, "", indent_unit)
                    )),
                    insert_text: expand(&body, &indent, indent_unit),
                    replace: Some(TextRange::on_line(pos.line, start as u32, caret as u32)),
                });
            }
        }
        return items;
    }

    if word.is_empty() || (word_start > 0 && !chars[..word_start].iter().all(|c| c.is_whitespace()))
    {
        // Live templates only at the start of a statement.
        return items;
    }
    for template in live_templates(language) {
        if template.abbreviation.starts_with(&word) {
            items.push(CompletionItem {
                label: template.abbreviation.to_string(),
                kind: SymbolKind::Snippet,
                detail: Some(template.description.to_string()),
                documentation: Some(format!(
                    "Live template\n\n```\n{}\n```",
                    expand(template.body, "", indent_unit)
                )),
                insert_text: expand(template.body, &indent, indent_unit),
                replace: Some(TextRange::on_line(
                    pos.line,
                    word_start as u32,
                    caret as u32,
                )),
            });
        }
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_templates_expand_with_the_line_indentation() {
        let text = "fn a() {\n    te\n}";
        let items = completions("rust", text, TextPos::new(1, 6), "    ");
        let test = items.iter().find(|i| i.label == "test").unwrap();
        assert_eq!(
            test.insert_text,
            "#[test]\n    fn name() {\n        \n    }"
        );
        assert_eq!(test.replace, Some(TextRange::on_line(1, 4, 6)));
        assert!(items.iter().all(|i| i.kind == SymbolKind::Snippet));
    }

    #[test]
    fn live_templates_only_start_a_statement() {
        let items = completions("rust", "let x = te", TextPos::new(0, 10), "    ");
        assert!(items.is_empty());
    }

    #[test]
    fn postfix_templates_wrap_the_expression() {
        let text = "    config.items().len().i";
        let items = completions("rust", text, TextPos::new(0, 26), "    ");
        let item = items.iter().find(|i| i.label == ".if").unwrap();
        assert_eq!(
            item.insert_text,
            "if config.items().len() {\n        \n    }"
        );
        assert_eq!(item.replace, Some(TextRange::on_line(0, 4, 26)));

        let items = completions("python", "    foo(a, b).pr", TextPos::new(0, 16), "    ");
        assert_eq!(items[0].insert_text, "print(foo(a, b))");
        assert_eq!(items[0].replace, Some(TextRange::on_line(0, 4, 16)));
    }

    #[test]
    fn postfix_stops_at_operators_and_open_parens() {
        let items = completions("rust", "call(x + y.no", TextPos::new(0, 13), "    ");
        let item = items.iter().find(|i| i.label == ".not").unwrap();
        assert_eq!(item.insert_text, "!y");
        assert!(completions("markdown", "a.if", TextPos::new(0, 4), "  ").is_empty());
    }
}
