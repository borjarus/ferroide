//! Bridges from `LanguageService` to GPUI Kit's editor LSP hooks:
//! completion, hover and code actions. The service works in FerroIDE's own
//! types (0-based lines, `char` columns); GPUI Kit speaks `lsp_types`.
//!
//! Requests run on the background executor; the editor replaces its pending
//! request task whenever a newer one starts, which cancels the old one.

use std::{path::PathBuf, rc::Rc, sync::Arc};

use anyhow::Result;
use ferro_services::{
    CompletionItem, HoverInfo, LanguageService, QuickFix, QuickFixKind, SymbolKind, TextEdit,
    TextPos, TextRange,
};
use gpui_kit::{
    App, AppContext as _, Entity, SharedString, Task, Window,
    component::input::{
        CodeActionProvider, CompletionProvider, EditorState, HoverProvider, Rope, RopeExt as _,
    },
};
use lsp_types::{
    CodeAction, CodeActionKind, CompletionContext, CompletionItemKind, CompletionResponse,
    Documentation, Hover, HoverContents, MarkupContent, MarkupKind,
};

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

/// Quick fixes as code actions; the edits travel in `data` and are applied
/// by [`LanguageProviders::perform_code_action`]. Preferred fixes first.
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
            data: serde_json::to_value(lsp_edits(&fix.edits)).ok(),
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
        let request = self
            .language
            .completions(&self.path, &text.to_string(), pos);
        cx.background_spawn(async move {
            let items = request.await?;
            Ok(CompletionResponse::Array(
                items.iter().map(completion_item).collect(),
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
        let edits: Vec<lsp_types::TextEdit> = action
            .data
            .and_then(|data| serde_json::from_value(data).ok())
            .unwrap_or_default();
        state.update(cx, |state, cx| state.apply_lsp_edits(&edits, window, cx));
        Task::ready(Ok(()))
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
        };
        let actions = code_actions(&[fix("other", false), fix("best", true)]);
        assert_eq!(actions[0].title, "best");
        let edits: Vec<lsp_types::TextEdit> =
            serde_json::from_value(actions[0].data.clone().unwrap()).unwrap();
        assert_eq!(edits[0].range.start.line, 1);
        assert_eq!(edits[0].new_text, "x");
    }
}
