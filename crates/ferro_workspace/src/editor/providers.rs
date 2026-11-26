//! Bridges from `LanguageService` to GPUI Kit's editor LSP hooks:
//! completion, hover, code actions, Go to Definition (Ctrl+click and the
//! underline while hovering with Ctrl) and semantic highlighting. The service works in FerroIDE's own
//! types (0-based lines, `char` columns); GPUI Kit speaks `lsp_types`.
//!
//! Requests run on the background executor; the editor replaces its pending
//! request task whenever a newer one starts, which cancels the old one.

use std::{path::PathBuf, rc::Rc, sync::Arc};

use anyhow::Result;
use ferro_services::{
    CompletionItem, GotoKind, HoverInfo, LanguageService, QuickFix, QuickFixKind, SemanticToken,
    SymbolKind, TextEdit, TextPos, TextRange,
};
use gpui_kit::{
    App, AppContext as _, Entity, SharedString, Task, Window,
    component::input::{
        CodeActionProvider, CompletionProvider, DefinitionProvider,
        DocumentRangeSemanticTokensProvider, EditorState, HoverProvider, Rope, RopeExt as _,
        ShowDocumentHandler,
    },
};
use lsp_types::{
    CodeAction, CodeActionKind, CompletionContext, CompletionItemKind, CompletionResponse,
    Documentation, Hover, HoverContents, MarkupContent, MarkupKind, SemanticTokens,
    SemanticTokensLegend,
};
use serde::{Deserialize, Serialize};

use crate::requests::{ApplyWorkspaceEdit, OpenLocation};

/// The provider id GPUI Kit tags code actions with.
pub const CODE_ACTION_PROVIDER_ID: &str = "ferro.language";

pub fn to_lsp_pos(pos: TextPos) -> lsp_types::Position {
    lsp_types::Position::new(pos.line, pos.column)
}

pub fn from_lsp_pos(pos: lsp_types::Position) -> TextPos {
    TextPos::new(pos.line, pos.character)
}

pub fn to_lsp_range(range: TextRange) -> lsp_types::Range {
    lsp_types::Range::new(to_lsp_pos(range.start), to_lsp_pos(range.end))
}

fn kind(kind: SymbolKind) -> CompletionItemKind {
    match kind {
        SymbolKind::Module => CompletionItemKind::MODULE,
        SymbolKind::Struct => CompletionItemKind::STRUCT,
        SymbolKind::Enum => CompletionItemKind::ENUM,
        SymbolKind::Trait => CompletionItemKind::INTERFACE,
        SymbolKind::Function | SymbolKind::Test => CompletionItemKind::FUNCTION,
        SymbolKind::Method => CompletionItemKind::METHOD,
        SymbolKind::Field => CompletionItemKind::FIELD,
        SymbolKind::Variable => CompletionItemKind::VARIABLE,
        SymbolKind::Constant => CompletionItemKind::CONSTANT,
        SymbolKind::Keyword => CompletionItemKind::KEYWORD,
        SymbolKind::Snippet | SymbolKind::Macro => CompletionItemKind::SNIPPET,
    }
}

/// A completion item as GPUI Kit's editor inserts it: a text edit over
/// `replace`, by default the word before the caret (`word_start..caret`).
pub fn completion_item_at(
    item: &CompletionItem,
    word_start: TextPos,
    caret: TextPos,
) -> lsp_types::CompletionItem {
    let range = item.replace.unwrap_or(TextRange::new(word_start, caret));
    lsp_types::CompletionItem {
        text_edit: Some(lsp_types::CompletionTextEdit::Edit(
            lsp_types::TextEdit::new(to_lsp_range(range), item.insert_text.clone()),
        )),
        filter_text: Some(item.label.trim_start_matches('.').to_string()),
        ..completion_item(item)
    }
}

pub fn completion_item(item: &CompletionItem) -> lsp_types::CompletionItem {
    lsp_types::CompletionItem {
        label: item.label.clone(),
        kind: Some(kind(item.kind)),
        detail: item.detail.clone(),
        documentation: item.documentation.clone().map(|value| {
            Documentation::MarkupContent(MarkupContent {
                kind: MarkupKind::Markdown,
                value,
            })
        }),
        insert_text: Some(item.insert_text.clone()),
        ..Default::default()
    }
}

pub fn hover(info: &HoverInfo) -> Hover {
    Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: info.contents.clone(),
        }),
        range: Some(to_lsp_range(info.range)),
    }
}

fn lsp_edits(edits: &[TextEdit]) -> Vec<lsp_types::TextEdit> {
    edits
        .iter()
        .map(|e| lsp_types::TextEdit::new(to_lsp_range(e.range), e.new_text.clone()))
        .collect()
}

/// What a code action carries to [`LanguageProviders::perform_code_action`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FixData {
    pub edits: Vec<lsp_types::TextEdit>,
    /// The service's token for resolving the fix ([`QuickFix::data`]).
    pub token: Option<String>,
}

/// Quick fixes as code actions; the edits and the resolve token travel in
/// `data` and are applied by [`LanguageProviders::perform_code_action`].
/// Preferred fixes first.
pub fn code_actions(fixes: &[QuickFix]) -> Vec<CodeAction> {
    let mut fixes: Vec<&QuickFix> = fixes.iter().collect();
    fixes.sort_by_key(|f| !f.preferred);
    fixes
        .into_iter()
        .map(|fix| CodeAction {
            title: fix.title.clone(),
            kind: Some(match fix.kind {
                QuickFixKind::QuickFix => CodeActionKind::QUICKFIX,
                QuickFixKind::Refactor => CodeActionKind::REFACTOR,
                QuickFixKind::Intention => CodeActionKind::REFACTOR_REWRITE,
            }),
            is_preferred: Some(fix.preferred),
            data: serde_json::to_value(FixData {
                edits: lsp_edits(&fix.edits),
                token: fix.data.clone(),
            })
            .ok(),
            ..Default::default()
        })
        .collect()
}

/// The language service, as GPUI Kit's editor providers, for one file.
pub struct LanguageProviders {
    pub path: PathBuf,
    pub language: Arc<dyn LanguageService>,
}

impl LanguageProviders {
    pub fn new(path: PathBuf, language: Arc<dyn LanguageService>) -> Rc<Self> {
        Rc::new(Self { path, language })
    }
}

impl CompletionProvider for LanguageProviders {
    fn completions(
        &self,
        text: &Rope,
        offset: usize,
        _: CompletionContext,
        _: &mut Window,
        cx: &mut App,
    ) -> Task<Result<CompletionResponse>> {
        let pos = from_lsp_pos(text.offset_to_position(offset));
        let content = text.to_string();
        let word_start = {
            let line = content.lines().nth(pos.line as usize).unwrap_or_default();
            let chars: Vec<char> = line.chars().collect();
            let caret = (pos.column as usize).min(chars.len());
            let start = chars[..caret]
                .iter()
                .rposition(|c| !(c.is_alphanumeric() || *c == '_'))
                .map_or(0, |i| i + 1);
            TextPos::new(pos.line, start as u32)
        };
        let request = self.language.completions(&self.path, &content, pos);
        cx.background_spawn(async move {
            let items = request.await?;
            Ok(CompletionResponse::Array(
                items
                    .iter()
                    .map(|item| completion_item_at(item, word_start, pos))
                    .collect(),
            ))
        })
    }

    fn is_completion_trigger(&self, _: usize, new_text: &str, _: &mut App) -> bool {
        new_text
            .chars()
            .last()
            .is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '.' || c == ':')
    }
}

impl HoverProvider for LanguageProviders {
    fn hover(
        &self,
        text: &Rope,
        offset: usize,
        _: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Option<Hover>>> {
        let pos = from_lsp_pos(text.offset_to_position(offset));
        let request = self.language.hover(&self.path, &text.to_string(), pos);
        cx.background_spawn(async move { Ok(request.await?.as_ref().map(hover)) })
    }
}

impl CodeActionProvider for LanguageProviders {
    fn id(&self) -> SharedString {
        CODE_ACTION_PROVIDER_ID.into()
    }

    fn code_actions(
        &self,
        state: Entity<EditorState>,
        range: std::ops::Range<usize>,
        _: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Vec<CodeAction>>> {
        let state = state.read(cx);
        let text = state.text();
        let range = TextRange::new(
            from_lsp_pos(text.offset_to_position(range.start)),
            from_lsp_pos(text.offset_to_position(range.end)),
        );
        let request = self
            .language
            .code_actions(&self.path, &text.to_string(), range);
        cx.background_spawn(async move { Ok(code_actions(&request.await?)) })
    }

    fn perform_code_action(
        &self,
        state: Entity<EditorState>,
        action: CodeAction,
        _push_to_history: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<()>> {
        let data: FixData = action
            .data
            .and_then(|data| serde_json::from_value(data).ok())
            .unwrap_or_default();
        let fix = QuickFix {
            title: action.title,
            kind: QuickFixKind::QuickFix,
            edits: data
                .edits
                .iter()
                .map(|e| TextEdit {
                    range: TextRange::new(from_lsp_pos(e.range.start), from_lsp_pos(e.range.end)),
                    new_text: e.new_text.clone(),
                })
                .collect(),
            preferred: false,
            data: data.token,
        };
        let text = state.read(cx).text().to_string();
        let request = self.language.resolve_quick_fix(&self.path, &text, &fix);
        let path = self.path.clone();
        window.spawn(cx, async move |cx| {
            let edit = request.await?;
            cx.update(|window, cx| {
                // The text changed while the fix was computed.
                if *state.read(cx).text() != text {
                    return;
                }
                let mut others = Vec::new();
                for file in edit.files {
                    let edits: Vec<TextEdit> = file.edits.into_iter().map(|e| e.edit).collect();
                    if same_path(&file.path, &path) {
                        let edits: Vec<lsp_types::TextEdit> =
                            super::navigation::sorted_from_end(&edits)
                                .iter()
                                .map(|e| {
                                    lsp_types::TextEdit::new(
                                        to_lsp_range(e.range),
                                        e.new_text.clone(),
                                    )
                                })
                                .collect();
                        state.update(cx, |state, cx| state.apply_lsp_edits(&edits, window, cx));
                    } else {
                        others.push((file.path, edits));
                    }
                }
                if !others.is_empty() {
                    window.dispatch_action(Box::new(ApplyWorkspaceEdit { files: others }), cx);
                }
            })?;
            Ok(())
        })
    }
}

/// Whether two paths name the same file (drive letter case, verbatim
/// prefix).
pub fn same_path(a: &std::path::Path, b: &std::path::Path) -> bool {
    a == b || ferro_lsp::uri::normalize(a) == ferro_lsp::uri::normalize(b)
}

impl DefinitionProvider for LanguageProviders {
    fn definitions(
        &self,
        text: &Rope,
        offset: usize,
        _: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Vec<lsp_types::LocationLink>>> {
        let pos = from_lsp_pos(text.offset_to_position(offset));
        let request = self
            .language
            .goto(&self.path, &text.to_string(), pos, GotoKind::Definition);
        cx.background_spawn(async move {
            Ok(request
                .await?
                .into_iter()
                .map(|location| {
                    let range = to_lsp_range(location.range);
                    lsp_types::LocationLink {
                        origin_selection_range: None,
                        target_uri: ferro_lsp::uri::lsp_uri(&location.path),
                        target_range: range,
                        target_selection_range: range,
                    }
                })
                .collect())
        })
    }
}

/// Go to Definition targets open through the workspace (other files, and
/// navigation history for this one).
pub fn show_document_handler() -> ShowDocumentHandler {
    Rc::new(|params, window, cx| {
        let Some(path) = ferro_lsp::uri::uri_to_path(params.uri.as_str()) else {
            return false;
        };
        let position = params
            .selection
            .map(|range| from_lsp_pos(range.start))
            .unwrap_or_default();
        window.dispatch_action(Box::new(OpenLocation::at(path, position)), cx);
        true
    })
}

/// Theme highlight names semantic tokens are drawn with.
const SEMANTIC_STYLES: &[&str] = &[
    "type",
    "enum",
    "variant",
    "function",
    "property",
    "keyword",
    "comment",
    "string",
    "number",
    "string.regex",
    "operator",
    "preproc",
    "constant",
    "variable",
];

/// The theme style of an LSP token, or `None` to keep tree-sitter's.
fn semantic_style(token: &SemanticToken) -> Option<&'static str> {
    let constant = token
        .modifiers
        .iter()
        .any(|m| m == "readonly" || m == "constant" || m == "static");
    Some(match token.kind.as_str() {
        "type" | "class" | "struct" | "interface" | "typeParameter" | "typeAlias" => "type",
        "enum" => "enum",
        "enumMember" => "variant",
        "function" | "method" => "function",
        "macro" | "decorator" | "attribute" => "preproc",
        "property" => "property",
        "keyword" | "modifier" => "keyword",
        "comment" => "comment",
        "string" => "string",
        "number" => "number",
        "regexp" => "string.regex",
        "operator" => "operator",
        "variable" if constant => "constant",
        "parameter" => "variable",
        _ => return None,
    })
}

/// Tokens in absolute positions → the relative encoding of `SEMANTIC_STYLES`.
pub fn encode_semantic_tokens(mut tokens: Vec<SemanticToken>) -> SemanticTokens {
    tokens.sort_by_key(|t| (t.line, t.start));
    let mut data = Vec::with_capacity(tokens.len());
    let (mut line, mut start) = (0, 0);
    for token in &tokens {
        let Some(style) = semantic_style(token) else {
            continue;
        };
        let Some(index) = SEMANTIC_STYLES.iter().position(|s| *s == style) else {
            continue;
        };
        let delta_line = token.line - line;
        let delta_start = if delta_line == 0 {
            token.start - start
        } else {
            token.start
        };
        data.push(lsp_types::SemanticToken {
            delta_line,
            delta_start,
            length: token.length,
            token_type: index as u32,
            token_modifiers_bitset: 0,
        });
        (line, start) = (token.line, token.start);
    }
    SemanticTokens {
        result_id: None,
        data,
    }
}

impl DocumentRangeSemanticTokensProvider for LanguageProviders {
    fn legend(&self) -> SemanticTokensLegend {
        SemanticTokensLegend {
            token_types: SEMANTIC_STYLES
                .iter()
                .map(|s| lsp_types::SemanticTokenType::new(s))
                .collect(),
            token_modifiers: Vec::new(),
        }
    }

    fn semantic_tokens(
        &self,
        text: &Rope,
        _: std::ops::Range<usize>,
        _: &mut Window,
        cx: &mut App,
    ) -> Task<Result<SemanticTokens>> {
        let request = self.language.semantic_tokens(&self.path, &text.to_string());
        cx.background_spawn(async move {
            Ok(encode_semantic_tokens(request.await?.unwrap_or_default()))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preferred_fixes_come_first_and_carry_edits() {
        let fix = |title: &str, preferred| QuickFix {
            title: title.into(),
            kind: QuickFixKind::QuickFix,
            edits: vec![TextEdit {
                range: TextRange::on_line(1, 0, 3),
                new_text: "x".into(),
            }],
            preferred,
            data: None,
        };
        let actions = code_actions(&[fix("other", false), fix("best", true)]);
        assert_eq!(actions[0].title, "best");
        let data: FixData = serde_json::from_value(actions[0].data.clone().unwrap()).unwrap();
        assert_eq!(data.edits[0].range.start.line, 1);
        assert_eq!(data.edits[0].new_text, "x");
    }

    #[test]
    fn completion_replaces_the_typed_word_or_its_own_range() {
        let item = CompletionItem {
            label: "HashMap".into(),
            kind: SymbolKind::Struct,
            detail: None,
            documentation: None,
            insert_text: "HashMap".into(),
            replace: None,
        };
        let lsp = completion_item_at(&item, TextPos::new(3, 4), TextPos::new(3, 7));
        let Some(lsp_types::CompletionTextEdit::Edit(edit)) = lsp.text_edit else {
            panic!("a text edit");
        };
        assert_eq!(edit.range, to_lsp_range(TextRange::on_line(3, 4, 7)));
        let postfix = CompletionItem {
            label: ".if".into(),
            replace: Some(TextRange::on_line(0, 0, 5)),
            insert_text: "if x {}".into(),
            ..item
        };
        let lsp = completion_item_at(&postfix, TextPos::new(0, 3), TextPos::new(0, 5));
        assert_eq!(lsp.filter_text.as_deref(), Some("if"));
        let Some(lsp_types::CompletionTextEdit::Edit(edit)) = lsp.text_edit else {
            panic!("a text edit");
        };
        assert_eq!(edit.range.start.character, 0);
    }

    #[test]
    fn semantic_tokens_encode_relative_to_the_theme_legend() {
        let token = |line, start, kind: &str| SemanticToken {
            line,
            start,
            length: 3,
            kind: kind.into(),
            modifiers: Vec::new(),
        };
        let encoded = encode_semantic_tokens(vec![
            token(2, 4, "function"),
            token(0, 1, "type"),
            token(0, 8, "variable"),
            token(2, 9, "macro"),
        ]);
        let data: Vec<(u32, u32, u32)> = encoded
            .data
            .iter()
            .map(|t| (t.delta_line, t.delta_start, t.token_type))
            .collect();
        // Plain variables keep tree-sitter's style.
        assert_eq!(data, [(0, 1, 0), (2, 4, 3), (0, 5, 11)]);
    }
}
