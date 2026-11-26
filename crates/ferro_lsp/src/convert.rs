//! LSP types → FerroIDE's service types.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use ferro_services::{
    CodeLens, CompletionItem, Diagnostic, DocumentLink, FileEdit, HierarchyItem, HoverInfo,
    InlayHint, InlayHintKind, Location, PreviewedEdit, QuickFixKind, SemanticToken, Severity,
    SignatureHelp, Symbol, SymbolKind, TextEdit, TextPos, TextRange, WorkspaceEdit,
};
use lsp_types as lsp;

use crate::{
    position::{Encoding, Lines},
    snippet,
    uri::{normalize, uri_to_path},
};

/// Texts of files a response mentions, for converting positions and
/// previews: the requesting document's current text, then what the server
/// has open, then the file on disk.
pub struct Texts<'a> {
    cache: HashMap<PathBuf, String>,
    open: &'a (dyn Fn(&Path) -> Option<String> + Sync),
}

impl<'a> Texts<'a> {
    pub fn new(
        path: &Path,
        text: &str,
        open: &'a (dyn Fn(&Path) -> Option<String> + Sync),
    ) -> Self {
        let mut cache = HashMap::new();
        cache.insert(normalize(path), text.to_string());
        Self { cache, open }
    }

    pub fn get(&mut self, path: &Path) -> &str {
        let key = normalize(path);
        if !self.cache.contains_key(&key) {
            let text = (self.open)(&key)
                .or_else(|| std::fs::read_to_string(&key).ok())
                .unwrap_or_default();
            self.cache.insert(key.clone(), text);
        }
        &self.cache[&key]
    }

    pub fn range(&mut self, path: &Path, range: lsp::Range, encoding: Encoding) -> TextRange {
        Lines::new(self.get(path)).range_from_lsp(range, encoding)
    }

    pub fn line(&mut self, path: &Path, line: u32) -> String {
        Lines::new(self.get(path)).line(line).to_string()
    }

    pub fn location(
        &mut self,
        uri: &lsp::Uri,
        range: lsp::Range,
        encoding: Encoding,
    ) -> Option<Location> {
        let path = uri_to_path(uri.as_str())?;
        let range = self.range(&path, range, encoding);
        Some(Location {
            line_text: self.line(&path, range.start.line),
            path,
            range,
        })
    }
}

pub fn symbol_kind(kind: lsp::SymbolKind) -> SymbolKind {
    match kind {
        lsp::SymbolKind::FILE
        | lsp::SymbolKind::MODULE
        | lsp::SymbolKind::NAMESPACE
        | lsp::SymbolKind::PACKAGE => SymbolKind::Module,
        lsp::SymbolKind::CLASS | lsp::SymbolKind::STRUCT | lsp::SymbolKind::OBJECT => {
            SymbolKind::Struct
        }
        lsp::SymbolKind::ENUM => SymbolKind::Enum,
        lsp::SymbolKind::INTERFACE => SymbolKind::Trait,
        lsp::SymbolKind::FUNCTION => SymbolKind::Function,
        lsp::SymbolKind::METHOD | lsp::SymbolKind::CONSTRUCTOR | lsp::SymbolKind::OPERATOR => {
            SymbolKind::Method
        }
        lsp::SymbolKind::FIELD | lsp::SymbolKind::PROPERTY | lsp::SymbolKind::KEY => {
            SymbolKind::Field
        }
        lsp::SymbolKind::CONSTANT
        | lsp::SymbolKind::ENUM_MEMBER
        | lsp::SymbolKind::NUMBER
        | lsp::SymbolKind::BOOLEAN
        | lsp::SymbolKind::STRING => SymbolKind::Constant,
        lsp::SymbolKind::TYPE_PARAMETER => SymbolKind::Struct,
        _ => SymbolKind::Variable,
    }
}

pub fn completion_kind(kind: Option<lsp::CompletionItemKind>) -> SymbolKind {
    use lsp::CompletionItemKind as K;
    match kind {
        Some(K::MODULE) | Some(K::FOLDER) | Some(K::FILE) => SymbolKind::Module,
        Some(K::CLASS) | Some(K::STRUCT) | Some(K::TYPE_PARAMETER) => SymbolKind::Struct,
        Some(K::ENUM) => SymbolKind::Enum,
        Some(K::INTERFACE) => SymbolKind::Trait,
        Some(K::FUNCTION) => SymbolKind::Function,
        Some(K::METHOD) | Some(K::CONSTRUCTOR) => SymbolKind::Method,
        Some(K::FIELD) | Some(K::PROPERTY) => SymbolKind::Field,
        Some(K::CONSTANT) | Some(K::ENUM_MEMBER) | Some(K::VALUE) => SymbolKind::Constant,
        Some(K::KEYWORD) => SymbolKind::Keyword,
        Some(K::SNIPPET) => SymbolKind::Snippet,
        _ => SymbolKind::Variable,
    }
}

pub fn severity(severity: Option<lsp::DiagnosticSeverity>) -> Severity {
    match severity {
        Some(lsp::DiagnosticSeverity::WARNING) => Severity::Warning,
        Some(lsp::DiagnosticSeverity::INFORMATION) => Severity::Info,
        Some(lsp::DiagnosticSeverity::HINT) => Severity::Hint,
        _ => Severity::Error,
    }
}

pub fn quick_fix_kind(kind: Option<&lsp::CodeActionKind>) -> QuickFixKind {
    match kind.map(|k| k.as_str()) {
        Some(k) if k.starts_with("quickfix") => QuickFixKind::QuickFix,
        Some(k) if k.starts_with("refactor.rewrite") || k.starts_with("source") => {
            QuickFixKind::Intention
        }
        Some(k) if k.starts_with("refactor") => QuickFixKind::Refactor,
        _ => QuickFixKind::QuickFix,
    }
}

fn marked_string(s: &lsp::MarkedString) -> String {
    match s {
        lsp::MarkedString::String(s) => s.clone(),
        lsp::MarkedString::LanguageString(ls) => format!("```{}\n{}\n```", ls.language, ls.value),
    }
}

pub fn hover_markdown(contents: &lsp::HoverContents) -> String {
    match contents {
        lsp::HoverContents::Scalar(s) => marked_string(s),
        lsp::HoverContents::Array(list) => list
            .iter()
            .map(marked_string)
            .collect::<Vec<_>>()
            .join("\n\n---\n\n"),
        lsp::HoverContents::Markup(markup) => markup.value.clone(),
    }
}

pub fn documentation(doc: &lsp::Documentation) -> String {
    match doc {
        lsp::Documentation::String(s) => s.clone(),
        lsp::Documentation::MarkupContent(m) => m.value.clone(),
    }
}

pub fn hover(
    hover: lsp::Hover,
    lines: &Lines,
    encoding: Encoding,
    pos: TextPos,
) -> Option<HoverInfo> {
    let contents = hover_markdown(&hover.contents);
    if contents.trim().is_empty() {
        return None;
    }
    Some(HoverInfo {
        contents,
        range: hover
            .range
            .map(|r| lines.range_from_lsp(r, encoding))
            .unwrap_or(TextRange::new(pos, pos)),
    })
}

pub fn diagnostics(
    path: &Path,
    server: &str,
    lines: &Lines,
    encoding: Encoding,
    list: Vec<lsp::Diagnostic>,
) -> Vec<Diagnostic> {
    list.into_iter()
        .map(|d| Diagnostic {
            path: path.to_path_buf(),
            range: lines.range_from_lsp(d.range, encoding),
            severity: severity(d.severity),
            message: d.message,
            source: d.source.unwrap_or_else(|| server.to_string()),
            code: d.code.map(|c| match c {
                lsp::NumberOrString::Number(n) => n.to_string(),
                lsp::NumberOrString::String(s) => s,
            }),
        })
        .collect()
}

/// A completion item, with its additional edits (auto-import) merged into
/// one replacement when they sit before or after the main edit.
pub fn completion(
    item: lsp::CompletionItem,
    text: &str,
    lines: &Lines,
    encoding: Encoding,
    caret: TextPos,
) -> CompletionItem {
    let is_snippet = item.insert_text_format == Some(lsp::InsertTextFormat::SNIPPET);
    let (mut insert, mut replace) = match &item.text_edit {
        Some(lsp::CompletionTextEdit::Edit(edit)) => (
            edit.new_text.clone(),
            Some(lines.range_from_lsp(edit.range, encoding)),
        ),
        Some(lsp::CompletionTextEdit::InsertAndReplace(edit)) => (
            edit.new_text.clone(),
            Some(lines.range_from_lsp(edit.replace, encoding)),
        ),
        None => (
            item.insert_text
                .clone()
                .unwrap_or_else(|| item.label.clone()),
            None,
        ),
    };
    if is_snippet {
        insert = snippet::to_text(&insert);
    }
    if let Some(extra) = &item.additional_text_edits
        && !extra.is_empty()
    {
        // Without a text edit the item replaces the word before the caret.
        let main = replace.unwrap_or_else(|| {
            let line: Vec<char> = lines.line(caret.line).chars().collect();
            let end = (caret.column as usize).min(line.len());
            let start = line[..end]
                .iter()
                .rposition(|c| !(c.is_alphanumeric() || *c == '_'))
                .map_or(0, |i| i + 1);
            TextRange::on_line(caret.line, start as u32, end as u32)
        });
        let edits: Vec<TextEdit> = extra
            .iter()
            .map(|e| TextEdit {
                range: lines.range_from_lsp(e.range, encoding),
                new_text: e.new_text.clone(),
            })
            .collect();
        if let Some((range, merged)) = merge_edits(text, main, &insert, &edits) {
            replace = Some(range);
            insert = merged;
        }
    }
    let detail = item
        .detail
        .clone()
        .or_else(|| {
            item.label_details
                .as_ref()
                .and_then(|d| d.description.clone())
        })
        .filter(|d| !d.is_empty());
    CompletionItem {
        label: item.label,
        kind: completion_kind(item.kind),
        detail,
        documentation: item.documentation.as_ref().map(documentation),
        insert_text: insert,
        replace,
    }
}

fn byte_offset(text: &str, pos: TextPos) -> usize {
    let mut offset = 0;
    for (i, line) in text.split('\n').enumerate() {
        if i as u32 == pos.line {
            let line = line.strip_suffix('\r').unwrap_or(line);
            return offset
                + line
                    .char_indices()
                    .nth(pos.column as usize)
                    .map_or(line.len(), |(b, _)| b);
        }
        offset += line.len() + 1;
    }
    text.len()
}

/// One edit equivalent to the main edit plus `extra` edits that do not
/// overlap it: the text between them is re-inserted unchanged.
pub fn merge_edits(
    text: &str,
    main: TextRange,
    main_text: &str,
    extra: &[TextEdit],
) -> Option<(TextRange, String)> {
    let mut all: Vec<(usize, usize, &str)> = extra
        .iter()
        .map(|e| {
            (
                byte_offset(text, e.range.start),
                byte_offset(text, e.range.end),
                e.new_text.as_str(),
            )
        })
        .collect();
    all.push((
        byte_offset(text, main.start),
        byte_offset(text, main.end),
        main_text,
    ));
    all.sort_by_key(|(start, end, _)| (*start, *end));
    for pair in all.windows(2) {
        if pair[0].1 > pair[1].0 {
            return None;
        }
    }
    let start = all.first()?.0;
    let end = all.last()?.1;
    let mut out = String::new();
    let mut at = start;
    for (s, e, new_text) in &all {
        out.push_str(text.get(at..*s)?);
        out.push_str(new_text);
        at = *e;
    }
    let pos = |byte: usize| {
        let before = &text[..byte];
        let line = before.matches('\n').count() as u32;
        let line_start = before.rfind('\n').map_or(0, |i| i + 1);
        TextPos::new(line, before[line_start..].chars().count() as u32)
    };
    Some((TextRange::new(pos(start), pos(end)), out))
}

pub fn signature_help(help: lsp::SignatureHelp) -> Option<SignatureHelp> {
    let active = help.active_signature.unwrap_or(0) as usize;
    let signature = help
        .signatures
        .get(active)
        .or_else(|| help.signatures.first())?
        .clone();
    let label = signature.label.clone();
    let parameters = signature
        .parameters
        .iter()
        .flatten()
        .filter_map(|p| match &p.label {
            lsp::ParameterLabel::Simple(name) => {
                let start = label.find(name.as_str())?;
                Some(start..start + name.len())
            }
            lsp::ParameterLabel::LabelOffsets([a, b]) => {
                let to_byte = |units: u32| {
                    let mut count = 0;
                    for (i, c) in label.char_indices() {
                        if count >= units {
                            return i;
                        }
                        count += c.len_utf16() as u32;
                    }
                    label.len()
                };
                Some(to_byte(*a)..to_byte(*b))
            }
        })
        .collect();
    let active_parameter = signature
        .active_parameter
        .or(help.active_parameter)
        .unwrap_or(0) as usize;
    Some(SignatureHelp {
        label,
        parameters,
        active_parameter,
        documentation: signature.documentation.as_ref().map(documentation),
    })
}

pub fn inlay_hint(hint: lsp::InlayHint, lines: &Lines, encoding: Encoding) -> InlayHint {
    let mut label = match hint.label {
        lsp::InlayHintLabel::String(s) => s,
        lsp::InlayHintLabel::LabelParts(parts) => parts.into_iter().map(|p| p.value).collect(),
    };
    if hint.padding_left == Some(true) {
        label.insert(0, ' ');
    }
    if hint.padding_right == Some(true) {
        label.push(' ');
    }
    InlayHint {
        position: lines.from_lsp(hint.position, encoding),
        kind: match hint.kind {
            Some(lsp::InlayHintKind::PARAMETER) => InlayHintKind::Parameter,
            _ if label.trim_start().starts_with("->") => InlayHintKind::Chaining,
            _ => InlayHintKind::Type,
        },
        label,
    }
}

pub fn document_symbols(
    response: lsp::DocumentSymbolResponse,
    path: &Path,
    lines: &Lines,
    encoding: Encoding,
) -> Vec<Symbol> {
    fn nested(
        symbol: lsp::DocumentSymbol,
        container: Option<String>,
        path: &Path,
        lines: &Lines,
        encoding: Encoding,
    ) -> Symbol {
        let name = symbol.name.clone();
        Symbol {
            kind: symbol_kind(symbol.kind),
            path: path.to_path_buf(),
            range: lines.range_from_lsp(symbol.range, encoding),
            selection: lines.from_lsp(symbol.selection_range.start, encoding),
            container,
            children: symbol
                .children
                .unwrap_or_default()
                .into_iter()
                .map(|child| nested(child, Some(name.clone()), path, lines, encoding))
                .collect(),
            name: symbol.name,
        }
    }
    match response {
        lsp::DocumentSymbolResponse::Nested(list) => list
            .into_iter()
            .map(|s| nested(s, None, path, lines, encoding))
            .collect(),
        lsp::DocumentSymbolResponse::Flat(list) => list
            .into_iter()
            .map(|s| {
                let range = lines.range_from_lsp(s.location.range, encoding);
                Symbol {
                    name: s.name,
                    kind: symbol_kind(s.kind),
                    path: path.to_path_buf(),
                    range,
                    selection: range.start,
                    container: s.container_name,
                    children: Vec::new(),
                }
            })
            .collect(),
    }
}

pub fn workspace_symbols(
    response: lsp::WorkspaceSymbolResponse,
    texts: &mut Texts,
    encoding: Encoding,
) -> Vec<Symbol> {
    let mut out = Vec::new();
    let mut push = |name: String,
                    kind: lsp::SymbolKind,
                    container: Option<String>,
                    uri: &lsp::Uri,
                    range: Option<lsp::Range>| {
        let Some(path) = uri_to_path(uri.as_str()) else {
            return;
        };
        let range = range
            .map(|r| texts.range(&path, r, encoding))
            .unwrap_or_default();
        out.push(Symbol {
            name,
            kind: symbol_kind(kind),
            path,
            range,
            selection: range.start,
            container,
            children: Vec::new(),
        });
    };
    match response {
        lsp::WorkspaceSymbolResponse::Flat(list) => {
            for s in list {
                push(
                    s.name,
                    s.kind,
                    s.container_name,
                    &s.location.uri,
                    Some(s.location.range),
                );
            }
        }
        lsp::WorkspaceSymbolResponse::Nested(list) => {
            for s in list {
                match s.location {
                    lsp::OneOf::Left(location) => push(
                        s.name,
                        s.kind,
                        s.container_name,
                        &location.uri,
                        Some(location.range),
                    ),
                    lsp::OneOf::Right(location) => {
                        push(s.name, s.kind, s.container_name, &location.uri, None)
                    }
                }
            }
        }
    }
    out
}

pub fn goto_locations(
    response: lsp::GotoDefinitionResponse,
    texts: &mut Texts,
    encoding: Encoding,
) -> Vec<Location> {
    let pairs: Vec<(lsp::Uri, lsp::Range)> = match response {
        lsp::GotoDefinitionResponse::Scalar(l) => vec![(l.uri, l.range)],
        lsp::GotoDefinitionResponse::Array(list) => {
            list.into_iter().map(|l| (l.uri, l.range)).collect()
        }
        lsp::GotoDefinitionResponse::Link(list) => list
            .into_iter()
            .map(|l| (l.target_uri, l.target_selection_range))
            .collect(),
    };
    pairs
        .iter()
        .filter_map(|(uri, range)| texts.location(uri, *range, encoding))
        .collect()
}

/// A workspace edit, with previews, grouped by file in order of appearance.
pub fn workspace_edit(
    edit: lsp::WorkspaceEdit,
    texts: &mut Texts,
    encoding: Encoding,
) -> WorkspaceEdit {
    let mut by_file: Vec<(PathBuf, Vec<lsp::TextEdit>)> = Vec::new();
    let mut add = |uri: &lsp::Uri, edits: Vec<lsp::TextEdit>| {
        let Some(path) = uri_to_path(uri.as_str()) else {
            return;
        };
        match by_file.iter_mut().find(|(p, _)| *p == path) {
            Some((_, list)) => list.extend(edits),
            None => by_file.push((path, edits)),
        }
    };
    if let Some(changes) = edit.document_changes {
        let ops = match changes {
            lsp::DocumentChanges::Edits(list) => list
                .into_iter()
                .map(lsp::DocumentChangeOperation::Edit)
                .collect(),
            lsp::DocumentChanges::Operations(list) => list,
        };
        for op in ops {
            match op {
                lsp::DocumentChangeOperation::Edit(e) => add(
                    &e.text_document.uri,
                    e.edits
                        .into_iter()
                        .map(|e| match e {
                            lsp::OneOf::Left(edit) => edit,
                            lsp::OneOf::Right(annotated) => annotated.text_edit,
                        })
                        .collect(),
                ),
                lsp::DocumentChangeOperation::Op(op) => {
                    // File creation, rename and deletion are not applied
                    // by the preview; the edits of the new file still are.
                    tracing::debug!("resource operation not applied: {op:?}");
                }
            }
        }
    } else if let Some(changes) = edit.changes {
        let mut list: Vec<_> = changes.into_iter().collect();
        list.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
        for (uri, edits) in list {
            add(&uri, edits);
        }
    }
    WorkspaceEdit {
        files: by_file
            .into_iter()
            .map(|(path, edits)| {
                let text = texts.get(&path).to_string();
                let lines = Lines::new(&text);
                let edits: Vec<TextEdit> = edits
                    .into_iter()
                    .map(|e| TextEdit {
                        range: lines.range_from_lsp(e.range, encoding),
                        new_text: e.new_text,
                    })
                    .collect();
                FileEdit {
                    edits: preview(&text, &edits),
                    path,
                }
            })
            .filter(|f| !f.edits.is_empty())
            .collect(),
    }
}

fn preview(text: &str, edits: &[TextEdit]) -> Vec<PreviewedEdit> {
    ferro_services::preview_edits(text, edits)
}

pub fn text_edits(edits: Vec<lsp::TextEdit>, lines: &Lines, encoding: Encoding) -> Vec<TextEdit> {
    edits
        .into_iter()
        .map(|e| TextEdit {
            range: lines.range_from_lsp(e.range, encoding),
            new_text: e.new_text,
        })
        .collect()
}

pub fn semantic_tokens(
    data: &[lsp::SemanticToken],
    legend: &lsp::SemanticTokensLegend,
    lines: &Lines,
    encoding: Encoding,
) -> Vec<SemanticToken> {
    let mut out = Vec::with_capacity(data.len());
    let (mut line, mut start) = (0u32, 0u32);
    for token in data {
        if token.delta_line > 0 {
            line += token.delta_line;
            start = token.delta_start;
        } else {
            start += token.delta_start;
        }
        let Some(kind) = legend.token_types.get(token.token_type as usize) else {
            continue;
        };
        let modifiers = legend
            .token_modifiers
            .iter()
            .enumerate()
            .filter(|(i, _)| *i < 32 && token.token_modifiers_bitset & (1 << i) != 0)
            .map(|(_, m)| m.as_str().to_string())
            .collect();
        let text = lines.line(line);
        let first = crate::position::lsp_to_char_column(text, start, encoding);
        let last = crate::position::lsp_to_char_column(text, start + token.length, encoding);
        out.push(SemanticToken {
            line,
            start: first,
            length: last.saturating_sub(first),
            kind: kind.as_str().to_string(),
            modifiers,
        });
    }
    out
}

pub fn code_lens(lens: lsp::CodeLens, lines: &Lines, encoding: Encoding) -> Option<CodeLens> {
    let command = lens.command?;
    Some(CodeLens {
        range: lines.range_from_lsp(lens.range, encoding),
        title: command.title,
        command: Some(command.command).filter(|c| !c.is_empty()),
    })
}

pub fn document_link(
    link: lsp::DocumentLink,
    lines: &Lines,
    encoding: Encoding,
) -> Option<DocumentLink> {
    Some(DocumentLink {
        range: lines.range_from_lsp(link.range, encoding),
        target: link.target?.as_str().to_string(),
    })
}

/// A call hierarchy item (type hierarchy items have the same fields).
pub fn hierarchy_item(
    item: lsp::CallHierarchyItem,
    texts: &mut Texts,
    encoding: Encoding,
) -> Option<HierarchyItem> {
    let path = uri_to_path(item.uri.as_str())?;
    Some(HierarchyItem {
        name: item.name,
        kind: symbol_kind(item.kind),
        detail: item.detail,
        range: texts.range(&path, item.range, encoding),
        selection: texts.range(&path, item.selection_range, encoding),
        path,
    })
}

pub fn type_hierarchy_item(
    item: lsp::TypeHierarchyItem,
    texts: &mut Texts,
    encoding: Encoding,
) -> Option<HierarchyItem> {
    hierarchy_item(
        lsp::CallHierarchyItem {
            name: item.name,
            kind: item.kind,
            tags: item.tags.map(|t| vec![t]),
            detail: item.detail,
            uri: item.uri,
            range: item.range,
            selection_range: item.selection_range,
            data: item.data,
        },
        texts,
        encoding,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_import_edits_merge_into_one_replacement() {
        let text = "use a::B;\n\nfn main() {\n    Has\n}\n";
        let main = TextRange::on_line(3, 4, 7);
        let extra = [TextEdit {
            range: TextRange::on_line(1, 0, 0),
            new_text: "use std::collections::HashMap;\n".into(),
        }];
        let (range, merged) = merge_edits(text, main, "HashMap", &extra).unwrap();
        assert_eq!(
            range,
            TextRange::new(TextPos::new(1, 0), TextPos::new(3, 7))
        );
        assert_eq!(
            merged,
            "use std::collections::HashMap;\n\nfn main() {\n    HashMap"
        );
        let overlapping = [TextEdit {
            range: TextRange::on_line(3, 5, 6),
            new_text: "x".into(),
        }];
        assert!(merge_edits(text, main, "HashMap", &overlapping).is_none());
    }

    #[test]
    fn signature_parameters_from_offsets_and_names() {
        let help = lsp::SignatureHelp {
            signatures: vec![lsp::SignatureInformation {
                label: "fn greet(name: &str, times: u8)".into(),
                documentation: None,
                parameters: Some(vec![
                    lsp::ParameterInformation {
                        label: lsp::ParameterLabel::LabelOffsets([9, 19]),
                        documentation: None,
                    },
                    lsp::ParameterInformation {
                        label: lsp::ParameterLabel::Simple("times: u8".into()),
                        documentation: None,
                    },
                ]),
                active_parameter: None,
            }],
            active_signature: Some(0),
            active_parameter: Some(1),
        };
        let help = signature_help(help).unwrap();
        assert_eq!(&help.label[help.parameters[0].clone()], "name: &str");
        assert_eq!(&help.label[help.parameters[1].clone()], "times: u8");
        assert_eq!(help.active_parameter, 1);
    }

    #[test]
    fn semantic_tokens_decode_relative_positions() {
        let legend = lsp::SemanticTokensLegend {
            token_types: vec!["function".into(), "variable".into()],
            token_modifiers: vec!["declaration".into(), "readonly".into()],
        };
        let data = [
            lsp::SemanticToken {
                delta_line: 0,
                delta_start: 3,
                length: 4,
                token_type: 0,
                token_modifiers_bitset: 1,
            },
            lsp::SemanticToken {
                delta_line: 1,
                delta_start: 6,
                length: 1,
                token_type: 1,
                token_modifiers_bitset: 3,
            },
            lsp::SemanticToken {
                delta_line: 0,
                delta_start: 4,
                length: 2,
                token_type: 9,
                token_modifiers_bitset: 0,
            },
        ];
        let text = "fn main() {\n  let x = 1;\n}";
        let tokens = semantic_tokens(&data, &legend, &Lines::new(text), Encoding::Utf16);
        assert_eq!(tokens.len(), 2, "unknown token types are skipped");
        assert_eq!(
            (tokens[0].line, tokens[0].start, tokens[0].length),
            (0, 3, 4)
        );
        assert_eq!(tokens[0].modifiers, ["declaration"]);
        assert_eq!((tokens[1].line, tokens[1].start), (1, 6));
        assert_eq!(tokens[1].modifiers, ["declaration", "readonly"]);
    }

    #[test]
    fn snippet_completions_insert_plain_text() {
        let item = lsp::CompletionItem {
            label: "println!".into(),
            insert_text: Some("println!(\"$1\")$0".into()),
            insert_text_format: Some(lsp::InsertTextFormat::SNIPPET),
            kind: Some(lsp::CompletionItemKind::FUNCTION),
            ..Default::default()
        };
        let text = "pri";
        let item = completion(
            item,
            text,
            &Lines::new(text),
            Encoding::Utf16,
            TextPos::new(0, 3),
        );
        assert_eq!(item.insert_text, "println!(\"\")");
        assert_eq!(item.kind, SymbolKind::Function);
        assert_eq!(item.replace, None);
    }
}
