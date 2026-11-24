//! Print the syntax tree of a file: `cargo run -p ferro_languages --example sexp -- <file>`.
fn main() {
    let path = std::env::args().nth(1).expect("file");
    let text = std::fs::read_to_string(&path).unwrap();
    let registry = ferro_languages::LanguageRegistry::shared_builtin();
    let pack = registry
        .detect(std::path::Path::new(&path), Some(&text))
        .expect("language");
    let tree = ferro_syntax::SyntaxTree::parse_with(
        pack.syntax().unwrap(),
        &ferro_text::Rope::from_str(&text),
        registry.resolver(),
    )
    .unwrap();
    println!("{}", tree.root().to_sexp());
    for layer in tree.injection_layers() {
        println!(
            "[{}] {:?}: {}",
            layer.language.id(),
            layer.ranges,
            layer.tree.root_node().to_sexp()
        );
    }
}
