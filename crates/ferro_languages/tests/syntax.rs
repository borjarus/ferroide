//! The syntax layer over the built-in Language Packs.

use std::{sync::Arc, time::Instant};

use ferro_languages::LanguageRegistry;
use ferro_syntax::{RunnableTag, SyntaxContext, SyntaxTree};
use ferro_text::Rope;

fn registry() -> Arc<LanguageRegistry> {
    LanguageRegistry::shared_builtin()
}

fn parse(language: &str, text: &str) -> SyntaxTree {
    let registry = registry();
    let syntax = registry.get(language).unwrap().syntax().unwrap();
    SyntaxTree::parse_with(syntax, &Rope::from_str(text), registry.resolver()).unwrap()
}

/// The highlight names covering the first occurrence of `needle`.
fn highlight_of(tree: &SyntaxTree, text: &str, needle: &str) -> Option<String> {
    let start = text
        .find(needle)
        .unwrap_or_else(|| panic!("{needle:?} not in sample"));
    tree.highlights(0..text.len())
        .into_iter()
        .find(|s| s.range.start <= start && start < s.range.end)
        .map(|s| s.name)
}

/// `(language, text, [(needle, highlight)])`.
type Sample = (
    &'static str,
    &'static str,
    &'static [(&'static str, &'static str)],
);

const SAMPLES: &[Sample] = &[
    (
        "rust",
        "// note\nfn main() { let s = \"hi\"; }\n",
        &[
            ("// note", "comment"),
            ("fn", "keyword"),
            ("\"hi\"", "string"),
        ],
    ),
    (
        "python",
        "# note\ndef f(x):\n    return 'hi'\n",
        &[
            ("# note", "comment"),
            ("def", "keyword"),
            ("'hi'", "string"),
        ],
    ),
    (
        "javascript",
        "// note\nfunction f() { return 'hi'; }\n",
        &[
            ("// note", "comment"),
            ("function", "keyword"),
            ("'hi'", "string"),
        ],
    ),
    (
        "typescript",
        "// note\ninterface A { x: number }\nconst s: string = 'hi';\n",
        &[
            ("// note", "comment"),
            ("interface", "keyword"),
            ("'hi'", "string"),
        ],
    ),
    (
        "tsx",
        "const e = <div className=\"a\">hi</div>;\n",
        &[("const", "keyword"), ("\"a\"", "string")],
    ),
    (
        "go",
        "// note\npackage main\nfunc main() { s := \"hi\" }\n",
        &[
            ("// note", "comment"),
            ("func", "keyword"),
            ("\"hi\"", "string"),
        ],
    ),
    (
        "java",
        "// note\nclass A { String s = \"hi\"; }\n",
        &[
            ("// note", "comment"),
            ("class", "keyword"),
            ("\"hi\"", "string"),
        ],
    ),
    (
        "c",
        "// note\nint main(void) { return 0; }\n",
        &[
            ("// note", "comment"),
            ("return", "keyword"),
            ("0", "number"),
        ],
    ),
    (
        "cpp",
        "// note\nclass A { public: int x; };\n",
        &[("// note", "comment"), ("class", "keyword")],
    ),
    (
        "csharp",
        "// note\nclass A { string s = \"hi\"; }\n",
        &[
            ("// note", "comment"),
            ("class", "keyword"),
            ("\"hi\"", "string"),
        ],
    ),
    (
        "html",
        "<!-- note -->\n<div class=\"a\">hi</div>\n",
        &[
            ("<!-- note -->", "comment"),
            ("div", "tag"),
            ("a\">", "string"),
        ],
    ),
    (
        "css",
        "/* note */\na { color: red; }\n",
        &[("/* note */", "comment"), ("color", "property")],
    ),
    (
        "json",
        "{\"a\": 1, \"b\": true}\n",
        &[("1", "number"), ("true", "constant")],
    ),
    (
        "toml",
        "# note\n[package]\nname = \"x\"\n",
        &[("# note", "comment"), ("\"x\"", "string")],
    ),
    (
        "yaml",
        "# note\nkey: 'value'\n",
        &[("# note", "comment"), ("'value'", "string")],
    ),
    ("markdown", "# Title\n\nText\n", &[("Title", "text.title")]),
    (
        "bash",
        "# note\necho \"hi\"\n",
        &[("# note", "comment"), ("\"hi\"", "string")],
    ),
    (
        "ruby",
        "# note\ndef f\n  'hi'\nend\n",
        &[
            ("# note", "comment"),
            ("def", "keyword"),
            ("'hi'", "string"),
        ],
    ),
    (
        "lua",
        "-- note\nlocal x = 'hi'\n",
        &[
            ("-- note", "comment"),
            ("local", "keyword"),
            ("'hi'", "string"),
        ],
    ),
    (
        "sql",
        "SELECT name FROM users WHERE id = 1;\n",
        &[("SELECT", "keyword"), ("FROM", "keyword")],
    ),
];

#[test]
fn fifteen_plus_languages_highlight() {
    assert!(SAMPLES.len() >= 15);
    for (language, text, expected) in SAMPLES {
        let tree = parse(language, text);
        assert!(
            !tree.has_errors(),
            "{language}: sample has syntax errors: {}",
            tree.root().to_sexp()
        );
        for (needle, name) in *expected {
            let got = highlight_of(&tree, text, needle);
            assert!(
                got.as_deref()
                    .is_some_and(|g| g == *name || g.starts_with(&format!("{name}."))),
                "{language}: {needle:?} highlighted as {got:?}, expected {name}"
            );
        }
    }
}

#[test]
fn highlights_do_not_overlap_and_respect_the_range() {
    let text = "fn main() {\n    let x = vec![1, 2, 3];\n    println!(\"{x:?}\");\n}\n".repeat(20);
    let tree = parse("rust", &text);
    let range = 100..400;
    let spans = tree.highlights(range.clone());
    assert!(!spans.is_empty());
    for pair in spans.windows(2) {
        assert!(pair[0].range.end <= pair[1].range.start, "{pair:?}");
    }
    assert!(
        spans
            .iter()
            .all(|s| range.start <= s.range.start && s.range.end <= range.end)
    );
}

#[test]
fn injected_languages_are_highlighted() {
    // HTML + JavaScript + CSS.
    let html = "<style>a { color: red; }</style>\n<script>function f() { return 1; }</script>\n";
    let tree = parse("html", html);
    let languages: Vec<&str> = tree
        .injection_layers()
        .iter()
        .map(|l| l.language.id())
        .collect();
    assert!(
        languages.contains(&"javascript") && languages.contains(&"css"),
        "{languages:?}"
    );
    assert_eq!(
        highlight_of(&tree, html, "function").as_deref(),
        Some("keyword")
    );
    assert_eq!(
        highlight_of(&tree, html, "color").as_deref(),
        Some("property")
    );

    // Code blocks in Markdown, by their info string (an alias).
    let md = "# Doc\n\n```py\ndef f():\n    pass\n```\n\nSome *text*.\n";
    let tree = parse("markdown", md);
    let languages: Vec<&str> = tree
        .injection_layers()
        .iter()
        .map(|l| l.language.id())
        .collect();
    assert!(languages.contains(&"python"), "{languages:?}");
    assert!(languages.contains(&"markdown_inline"), "{languages:?}");
    assert_eq!(highlight_of(&tree, md, "def").as_deref(), Some("keyword"));

    // SQL in strings.
    let py = "query = \"SELECT id FROM users\"\nother = \"hello\"\n";
    let tree = parse("python", py);
    assert_eq!(tree.injection_layers().len(), 1);
    assert_eq!(
        highlight_of(&tree, py, "SELECT").as_deref(),
        Some("keyword")
    );
    assert!(
        highlight_of(&tree, py, "hello")
            .unwrap()
            .starts_with("string")
    );

    let rs = "fn q() -> &'static str { r\"select * from t\" }\n";
    let tree = parse("rust", rs);
    assert_eq!(
        highlight_of(&tree, rs, "select").as_deref(),
        Some("keyword")
    );
}

#[test]
fn outline_nests_definitions() {
    let rust = "struct Config { a: u32 }\nimpl Config {\n    fn new() -> Self { todo!() }\n    fn get(&self) -> u32 { self.a }\n}\nfn main() {}\n";
    let outline = parse("rust", rust).outline();
    let names: Vec<(&str, &str)> = outline
        .iter()
        .map(|i| (i.name.as_str(), i.kind.as_str()))
        .collect();
    assert!(names.contains(&("Config", "struct")), "{names:?}");
    assert!(names.contains(&("Config", "impl")), "{names:?}");
    assert!(names.contains(&("main", "function")), "{names:?}");
    let methods: Vec<&str> = outline
        .iter()
        .flat_map(|i| i.children.iter().map(|c| c.name.as_str()))
        .collect();
    assert_eq!(
        methods,
        ["a", "new", "get"],
        "the field, then the methods in impl"
    );

    let python = "class A:\n    def m(self):\n        pass\n\ndef f():\n    pass\n";
    let outline = parse("python", python).outline();
    assert_eq!(outline.len(), 2);
    assert_eq!(outline[0].name, "A");
    assert_eq!(outline[0].children[0].name, "m");

    let md = "# One\n\ntext\n\n## Two\n\nmore\n\n# Three\n";
    let outline = parse("markdown", md).outline();
    let top: Vec<&str> = outline.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(top, ["One", "Three"]);
    assert_eq!(outline[0].children[0].name, "Two");

    let toml = "[package]\nname = \"x\"\n\n[dependencies]\nserde = \"1\"\n";
    let outline = parse("toml", toml).outline();
    let top: Vec<&str> = outline.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(top, ["package", "dependencies"]);
    assert_eq!(outline[1].children[0].name, "serde");

    // Languages with a tags query.
    for (language, text, name) in [
        ("go", "package main\nfunc Hello() {}\n", "Hello"),
        ("java", "class Greeter { void hi() {} }\n", "Greeter"),
        ("javascript", "function greet() {}\n", "greet"),
        (
            "typescript",
            "class Store { get(): number { return 1 } }\n",
            "Store",
        ),
        ("c", "int add(int a, int b) { return a + b; }\n", "add"),
        ("ruby", "class Greeter\n  def hi; end\nend\n", "Greeter"),
        ("bash", "deploy() {\n  echo hi\n}\n", "deploy"),
        ("json", "{\"name\": \"x\"}\n", "name"),
        ("yaml", "name: x\n", "name"),
        ("css", ".btn { color: red; }\n", ".btn"),
    ] {
        let outline = parse(language, text).outline();
        assert!(
            outline.iter().any(|i| i.name == name),
            "{language}: {name} not in {:?}",
            outline.iter().map(|i| &i.name).collect::<Vec<_>>()
        );
    }
}

#[test]
fn folds_follow_the_language() {
    let rust = "fn main() {\n    let x = 1;\n    let y = 2;\n}\n";
    let folds = parse("rust", rust).folds();
    assert_eq!(folds.len(), 1);
    assert_eq!((folds[0].start_line, folds[0].end_line), (0, 3));
    assert!(folds[0].closed_by_token);

    let python = "def f():\n    a = 1\n    b = 2\n\nx = 1\n";
    let folds = parse("python", python).folds();
    assert_eq!((folds[0].start_line, folds[0].end_line), (0, 2));
    assert!(
        !folds[0].closed_by_token,
        "Python bodies end without a token"
    );

    // No folds.scm: multi-line bracketed nodes.
    let sql = "SELECT a\nFROM t\nWHERE id IN (\n  1,\n  2\n);\n";
    let folds = parse("sql", sql).folds();
    assert!(
        folds.iter().any(|f| f.start_line == 2 && f.end_line == 5),
        "{folds:?}"
    );
}

#[test]
fn brackets_match_from_either_side() {
    let text = "fn f(a: u32) { if a > 1 { g([1, 2]); } }\n";
    let tree = parse("rust", text);
    let open = text.find('{').unwrap();
    let close = text.rfind('}').unwrap();
    let pair = tree.matching_brackets(open).unwrap();
    assert_eq!((pair.open.start, pair.close.start), (open, close));
    // Caret right after the closing brace.
    let pair = tree.matching_brackets(close + 1).unwrap();
    assert_eq!(pair.open.start, open);
    let bracket = text.find('[').unwrap();
    let pair = tree.matching_brackets(bracket).unwrap();
    assert_eq!(&text[pair.close.clone()], "]");
    assert!(
        tree.matching_brackets(text.find("a >").unwrap() + 1)
            .is_none()
    );

    let html = "<div><p>hi</p></div>\n";
    let tree = parse("html", html);
    assert!(tree.matching_brackets(0).is_some());
}

#[test]
fn smart_indentation() {
    let rust = "fn main() {\n    if x {\n\n    }\nlet y = 1;\n}\n";
    let tree = parse("rust", rust);
    assert_eq!(tree.indent_level(1), 1);
    assert_eq!(tree.indent_level(2), 2, "inside the if block");
    assert_eq!(tree.indent_level(3), 1, "closing brace");
    assert_eq!(tree.indent_level(4), 1, "statement in fn body");
    assert_eq!(tree.indent_level(5), 0);

    let python = "class A:\n    def f(self):\n        if x:\n            pass\n        else:\n            pass\n";
    let tree = parse("python", python);
    assert_eq!(tree.indent_level(1), 1);
    assert_eq!(tree.indent_level(3), 3);
    assert_eq!(tree.indent_level(4), 2, "else outdents");
    assert_eq!(tree.indent_level(5), 3);

    let go =
        "package main\n\nfunc f(x int) {\n\tswitch x {\n\tcase 1:\n\t\tg()\n\tdefault:\n\t}\n}\n";
    let tree = parse("go", go);
    assert_eq!(tree.indent_level(4), 1, "case at the switch's level");
    assert_eq!(tree.indent_level(5), 2, "case body");
    assert_eq!(tree.indent_level(7), 1, "switch's closing brace");

    let ruby = "def f\n  if x\n    1\n  else\n    2\n  end\nend\n";
    let tree = parse("ruby", ruby);
    assert_eq!(tree.indent_level(2), 2);
    assert_eq!(tree.indent_level(3), 1);
    assert_eq!(tree.indent_level(5), 1);
    assert_eq!(tree.indent_level(6), 0);
    assert_eq!(tree.indent_for_line(2, "  "), "    ");
}

#[test]
fn expand_selection_climbs_the_tree() {
    let text = "fn main() {\n    let total = add(1, 2);\n}\n";
    let tree = parse("rust", text);
    let caret = text.find("add").unwrap() + 1;
    let mut range = caret..caret;
    let mut steps = Vec::new();
    while let Some(next) = tree.expand_selection(range.clone()) {
        assert!(next.start <= range.start && range.end <= next.end && next != range);
        steps.push(text[next.clone()].to_string());
        range = next;
    }
    assert_eq!(steps[0], "add");
    assert!(steps.contains(&"add(1, 2)".to_string()), "{steps:?}");
    assert!(
        steps.contains(&"let total = add(1, 2);".to_string()),
        "{steps:?}"
    );
    assert_eq!(steps.last().unwrap(), text);

    // Inside an injection, the injected language's nodes come first.
    let html = "<script>let a = [1, 2];</script>\n";
    let tree = parse("html", html);
    let caret = html.find('1').unwrap();
    let first = tree.expand_selection(caret..caret).unwrap();
    assert_eq!(&html[first.clone()], "1");
    let second = tree.expand_selection(first).unwrap();
    assert_eq!(&html[second], "[1, 2]");
}

#[test]
fn runnables_from_queries() {
    let rust = "fn main() {}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn adds() {}\n    #[test]\n    #[ignore]\n    fn slow() {}\n    #[tokio::test]\n    async fn io() {}\n}\n";
    let runnables = parse("rust", rust).runnables();
    let found: Vec<(usize, RunnableTag, &str)> = runnables
        .iter()
        .map(|r| (r.line, r.tag, r.name.as_str()))
        .collect();
    assert_eq!(
        found,
        [
            (0, RunnableTag::Main, "main"),
            (3, RunnableTag::TestModule, "tests"),
            (5, RunnableTag::Test, "adds"),
            (8, RunnableTag::Test, "slow"),
            (10, RunnableTag::Test, "io"),
        ]
    );

    let python = "def test_add():\n    pass\n\nif __name__ == '__main__':\n    main()\n";
    let tags: Vec<RunnableTag> = parse("python", python)
        .runnables()
        .iter()
        .map(|r| r.tag)
        .collect();
    assert_eq!(tags, [RunnableTag::Test, RunnableTag::Main]);

    let go = "package main\nfunc main() {}\nfunc TestAdd(t *testing.T) {}\n";
    assert_eq!(parse("go", go).runnables().len(), 2);

    let js = "describe('math', () => {\n  it('adds', () => {});\n});\n";
    let runnables = parse("javascript", js).runnables();
    assert_eq!(runnables.len(), 2);
    assert_eq!(runnables[1].name, "adds");
}

#[test]
fn errors_and_context() {
    let text = "fn main() {\n    let x = \"text\"; // note\n    let y = ;\n}\n";
    let tree = parse("rust", text);
    let errors = tree.syntax_errors(10);
    assert!(!errors.is_empty());
    assert!(tree.line_of(errors[0].range.start) == 2);
    assert_eq!(
        tree.context_at(text.find("text").unwrap()),
        SyntaxContext::String
    );
    assert_eq!(
        tree.context_at(text.find("note").unwrap()),
        SyntaxContext::Comment
    );
    assert_eq!(
        tree.context_at(text.find("let x").unwrap()),
        SyntaxContext::Code
    );
}

/// Edits applied incrementally give the same tree as parsing from scratch.
#[test]
fn incremental_parsing_matches_a_fresh_parse() {
    let registry = registry();
    let syntax = registry.get("rust").unwrap().syntax().unwrap();
    let mut text = String::from("fn main() {\n    let a = 1;\n}\n\nstruct S { x: u32 }\n");
    let mut tree = SyntaxTree::parse(syntax.clone(), &Rope::from_str(&text)).unwrap();
    let edits: &[(usize, usize, &str)] = &[
        (16, 17, "value"),
        (0, 0, "// header\n"),
        (30, 30, "\n    let b = a + 2;"),
        (5, 9, ""),
        (10, 10, "{"),
        (10, 11, ""),
    ];
    for &(start, end, insert) in edits {
        let start = start.min(text.len());
        let end = end.min(text.len()).max(start);
        text.replace_range(start..end, insert);
        let changed = tree.reparse(&Rope::from_str(&text)).unwrap();
        assert!(changed.start <= start);
        let fresh = SyntaxTree::parse(syntax.clone(), &Rope::from_str(&text)).unwrap();
        assert_eq!(
            tree.root().to_sexp(),
            fresh.root().to_sexp(),
            "after {insert:?}"
        );
    }
    assert!(tree.reparse(&Rope::from_str(&text)).is_none(), "no change");
}

/// Plan Stage 5 DoD: incremental parsing < 2 ms per edit in a 10k-line file
/// (medium profile, optimized build).
#[test]
fn incremental_parse_of_a_ten_thousand_line_file_fits_the_budget() {
    let registry = registry();
    let syntax = registry.get("rust").unwrap().syntax().unwrap();
    // Shaped like real code: modules with a type and its methods.
    let method = "    /// Adds two numbers.\n    pub fn add_N(&self, b: u64) -> u64 {\n        let sum = self.value.wrapping_add(b);\n        if sum > 10 { sum - 1 } else { sum + 1 }\n    }\n\n";
    let mut text = String::new();
    let mut n = 0;
    while text.lines().count() < 10_000 {
        text.push_str(&format!(
            "pub mod m{n} {{\npub struct Counter{n} {{ value: u64 }}\n\nimpl Counter{n} {{\n"
        ));
        for _ in 0..20 {
            text.push_str(&method.replace('N', &n.to_string()));
            n += 1;
        }
        text.push_str("}\n}\n\n");
    }
    let mut rope = Rope::from_str(&text);
    let mut tree = SyntaxTree::parse(syntax, &rope).unwrap();
    let mut times = Vec::new();
    for i in 0..60 {
        // Type a character into a name in the middle of the file, then
        // delete it.
        let line = (5_000..)
            .map(|l| l + i * 7)
            .find(|&l| {
                rope.line(l, ferro_text::LINES)
                    .to_string()
                    .contains("let sum")
            })
            .unwrap();
        let at = rope.line_to_byte_idx(line, ferro_text::LINES) + 13;
        if i % 2 == 0 {
            rope.insert(at, "x");
        } else {
            rope.remove(at..at + 1);
        }
        let started = Instant::now();
        tree.reparse(&rope).unwrap();
        times.push(started.elapsed());
    }
    times.sort();
    let median = times[times.len() / 2];
    let budget_ms = if cfg!(debug_assertions) { 25.0 } else { 2.0 };
    eprintln!(
        "incremental reparse, 10k lines: median {median:?}, max {:?}",
        times.last().unwrap()
    );
    assert!(
        median.as_secs_f64() * 1000.0 < budget_ms,
        "median {median:?} over {budget_ms} ms"
    );
}
