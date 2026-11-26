//! An in-process language server for tests (feature `test-support`), speaking real JSON-RPC over
//! pipes. It answers from the text it was synchronized with, so tests also
//! check that incremental synchronization keeps both copies equal.
//!
//! - every `bad` is a published diagnostic;
//! - `fn name` lines are symbols and definitions;
//! - hovering `slow` never answers (cancellation tests);
//! - a change containing `CRASH` ends the server (crash tests).

use std::{
    collections::HashMap,
    io::{BufReader, pipe},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use crate::{
    Launcher, ServerConfig, Transport,
    position::{Encoding, apply_change},
    rpc::{read_message, write_message},
};
use lsp_types::TextDocumentContentChangeEvent;
use serde_json::{Value, json};

#[derive(Default)]
pub struct FakeState {
    pub launches: AtomicUsize,
    pub texts: Mutex<HashMap<String, String>>,
    pub configuration: Mutex<Option<Value>>,
    pub cancelled: Mutex<Vec<i64>>,
    pub watched: Mutex<Vec<Value>>,
    pub saved: Mutex<Vec<String>>,
    pub closed: Mutex<Vec<String>>,
    pub roots: Mutex<Vec<String>>,
    pub changes: AtomicUsize,
}

impl FakeState {
    pub fn text_of(&self, uri_suffix: &str) -> Option<String> {
        self.texts
            .lock()
            .unwrap()
            .iter()
            .find(|(uri, _)| uri.ends_with(uri_suffix))
            .map(|(_, t)| t.clone())
    }
}

pub struct FakeLauncher {
    pub state: Arc<FakeState>,
}

impl Launcher for FakeLauncher {
    fn launch(&self, config: &ServerConfig) -> std::io::Result<Transport> {
        let (client_reader, server_writer) = pipe()?;
        let (server_reader, client_writer) = pipe()?;
        self.state.launches.fetch_add(1, Ordering::SeqCst);
        self.state
            .roots
            .lock()
            .unwrap()
            .push(config.root.to_string_lossy().into_owned());
        let state = self.state.clone();
        std::thread::spawn(move || serve(state, BufReader::new(server_reader), server_writer));
        Ok(Transport {
            reader: Box::new(client_reader),
            writer: Box::new(client_writer),
            stderr: None,
            process: None,
        })
    }
}

fn utf16_col(line: &str, byte: usize) -> u32 {
    line[..byte].encode_utf16().count() as u32
}

/// `(line, start, end)` in UTF-16 of each whole-word occurrence of `word`.
fn occurrences(text: &str, word: &str) -> Vec<(u32, u32, u32)> {
    let mut out = Vec::new();
    for (i, line) in text.split('\n').enumerate() {
        let mut from = 0;
        while let Some(at) = line[from..].find(word) {
            let start = from + at;
            let end = start + word.len();
            let before = line[..start].chars().next_back();
            let after = line[end..].chars().next();
            let boundary = |c: Option<char>| c.is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
            if boundary(before) && boundary(after) {
                out.push((i as u32, utf16_col(line, start), utf16_col(line, end)));
            }
            from = end;
        }
    }
    out
}

fn word_at(text: &str, line: u32, character: u32) -> String {
    let line = text.split('\n').nth(line as usize).unwrap_or_default();
    let mut units = 0;
    let mut byte = line.len();
    for (b, c) in line.char_indices() {
        if units >= character {
            byte = b;
            break;
        }
        units += c.len_utf16() as u32;
    }
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let start = line[..byte]
        .rfind(|c: char| !is_word(c))
        .map_or(0, |i| i + 1);
    let end = line[byte..]
        .find(|c: char| !is_word(c))
        .map_or(line.len(), |i| byte + i);
    line[start..end].to_string()
}

fn range(line: u32, start: u32, end: u32) -> Value {
    json!({"start": {"line": line, "character": start}, "end": {"line": line, "character": end}})
}

fn diagnostics(uri: &str, text: &str) -> Value {
    let items: Vec<Value> = occurrences(text, "bad")
        .into_iter()
        .map(|(l, s, e)| json!({"range": range(l, s, e), "severity": 2, "message": "bad word", "source": "fake"}))
        .collect();
    json!({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics", "params": {"uri": uri, "diagnostics": items}})
}

fn serve(
    state: Arc<FakeState>,
    mut reader: BufReader<std::io::PipeReader>,
    mut writer: std::io::PipeWriter,
) {
    let send = |w: &mut std::io::PipeWriter, v: Value| {
        let _ = write_message(w, &v);
    };
    let mut pending_command: Option<Value> = None;
    while let Ok(Some(message)) = read_message(&mut reader) {
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let id = message.get("id").cloned();
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        // Responses to our own requests.
        if method.is_empty() {
            match id.as_ref().and_then(Value::as_i64) {
                Some(9001) => *state.configuration.lock().unwrap() = message.get("result").cloned(),
                Some(9003) => {
                    if let Some(id) = pending_command.take() {
                        send(
                            &mut writer,
                            json!({"jsonrpc": "2.0", "id": id, "result": null}),
                        );
                    }
                }
                _ => {}
            }
            continue;
        }
        let uri = params
            .pointer("/textDocument/uri")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let text = state
            .texts
            .lock()
            .unwrap()
            .get(&uri)
            .cloned()
            .unwrap_or_default();
        let position = params
            .get("position")
            .cloned()
            .unwrap_or(json!({"line": 0, "character": 0}));
        let (line, character) = (
            position["line"].as_u64().unwrap_or(0) as u32,
            position["character"].as_u64().unwrap_or(0) as u32,
        );
        let result: Value = match method.as_str() {
            "initialize" => json!({"capabilities": {
                "positionEncoding": "utf-16",
                "textDocumentSync": {"openClose": true, "change": 2, "save": {"includeText": false}},
                "completionProvider": {"triggerCharacters": ["."], "resolveProvider": true},
                "hoverProvider": true,
                "signatureHelpProvider": {"triggerCharacters": ["("]},
                "definitionProvider": true,
                "referencesProvider": true,
                "renameProvider": true,
                "documentFormattingProvider": true,
                "codeActionProvider": {"resolveProvider": true},
                "inlayHintProvider": true,
                "documentSymbolProvider": true,
                "workspaceSymbolProvider": true,
                "executeCommandProvider": {"commands": ["fake.fix"]},
                "semanticTokensProvider": {"legend": {"tokenTypes": ["keyword", "function"], "tokenModifiers": []}, "full": true},
            }}),
            "initialized" => {
                send(&mut writer, json!({"jsonrpc": "2.0", "id": 9001, "method": "workspace/configuration", "params": {"items": [{"section": "fake"}]}}));
                send(&mut writer, json!({"jsonrpc": "2.0", "id": 9002, "method": "client/registerCapability", "params": {"registrations": [
                    {"id": "w1", "method": "workspace/didChangeWatchedFiles", "registerOptions": {"watchers": [{"globPattern": "**/*.toml"}]}}
                ]}}));
                continue;
            }
            "textDocument/didOpen" => {
                let text = params.pointer("/textDocument/text").and_then(Value::as_str).unwrap_or_default().to_string();
                send(&mut writer, diagnostics(&uri, &text));
                state.texts.lock().unwrap().insert(uri, text);
                continue;
            }
            "textDocument/didChange" => {
                let mut text = text;
                for change in params["contentChanges"].as_array().cloned().unwrap_or_default() {
                    let change: TextDocumentContentChangeEvent = serde_json::from_value(change).unwrap();
                    text = apply_change(&text, &change, Encoding::Utf16);
                }
                state.changes.fetch_add(1, Ordering::SeqCst);
                if text.contains("CRASH") {
                    return;
                }
                send(&mut writer, diagnostics(&uri, &text));
                state.texts.lock().unwrap().insert(uri, text);
                continue;
            }
            "textDocument/didSave" => {
                state.saved.lock().unwrap().push(uri);
                continue;
            }
            "textDocument/didClose" => {
                state.closed.lock().unwrap().push(uri.clone());
                state.texts.lock().unwrap().remove(&uri);
                continue;
            }
            "workspace/didChangeWatchedFiles" => {
                state.watched.lock().unwrap().extend(params["changes"].as_array().cloned().unwrap_or_default());
                continue;
            }
            "$/cancelRequest" => {
                state.cancelled.lock().unwrap().push(params["id"].as_i64().unwrap_or(-1));
                continue;
            }
            "exit" => return,
            "shutdown" => Value::Null,
            "textDocument/hover" => {
                let word = word_at(&text, line, character);
                if word == "slow" {
                    continue;
                }
                json!({"contents": {"kind": "markdown", "value": format!("**{word}**")}})
            }
            "textDocument/completion" => json!({"isIncomplete": false, "items": [
                {"label": "HashMap", "kind": 7, "sortText": "a", "data": 1},
                {"label": "hello_world", "kind": 3, "insertText": "hello_world($1)$0", "insertTextFormat": 2, "sortText": "b"},
            ]}),
            "completionItem/resolve" => {
                let mut item = params.clone();
                if item["label"] == "HashMap" {
                    item["documentation"] = json!({"kind": "markdown", "value": "A hash map."});
                    item["additionalTextEdits"] = json!([{"range": range(0, 0, 0), "newText": "use std::collections::HashMap;\n"}]);
                }
                item
            }
            "textDocument/signatureHelp" => json!({"signatures": [{"label": "fn greet(name: &str)", "parameters": [{"label": [9, 19]}]}], "activeSignature": 0, "activeParameter": 0}),
            "textDocument/definition" => {
                let word = word_at(&text, line, character);
                let target = format!("fn {word}");
                match text.split('\n').enumerate().find(|(_, l)| l.contains(&target)) {
                    Some((i, l)) => {
                        let start = utf16_col(l, l.find(&target).unwrap() + 3);
                        json!([{"uri": uri, "range": range(i as u32, start, start + word.encode_utf16().count() as u32)}])
                    }
                    None => Value::Null,
                }
            }
            "textDocument/references" => {
                let word = word_at(&text, line, character);
                Value::Array(occurrences(&text, &word).into_iter().map(|(l, s, e)| json!({"uri": uri, "range": range(l, s, e)})).collect())
            }
            "textDocument/rename" => {
                let word = word_at(&text, line, character);
                let new_name = params["newName"].as_str().unwrap_or_default();
                let edits: Vec<Value> = occurrences(&text, &word).into_iter().map(|(l, s, e)| json!({"range": range(l, s, e), "newText": new_name})).collect();
                json!({"changes": {uri.clone(): edits}})
            }
            "textDocument/formatting" => {
                let lines = text.split('\n').count() as u32;
                let formatted: Vec<&str> = text.split('\n').map(str::trim_end).collect();
                json!([{"range": {"start": {"line": 0, "character": 0}, "end": {"line": lines, "character": 0}}, "newText": formatted.join("\n")}])
            }
            "textDocument/codeAction" => {
                let mut actions = vec![json!({"title": "Replace bad with good", "kind": "quickfix", "isPreferred": true, "data": {"uri": uri}})];
                actions.push(json!({"title": "Run fix command", "command": "fake.fix", "arguments": [uri]}));
                Value::Array(actions)
            }
            "codeAction/resolve" => {
                let mut action = params.clone();
                let target = params.pointer("/data/uri").and_then(Value::as_str).unwrap_or_default().to_string();
                let target_text = state.texts.lock().unwrap().get(&target).cloned().unwrap_or_default();
                let edits: Vec<Value> = occurrences(&target_text, "bad").into_iter().map(|(l, s, e)| json!({"range": range(l, s, e), "newText": "good"})).collect();
                action["edit"] = json!({"changes": {target: edits}});
                action
            }
            "workspace/executeCommand" => {
                let target = params["arguments"][0].as_str().unwrap_or_default().to_string();
                pending_command = id.clone();
                send(&mut writer, json!({"jsonrpc": "2.0", "id": 9003, "method": "workspace/applyEdit", "params": {"edit": {"changes": {target: [{"range": range(0, 0, 0), "newText": "// fixed\n"}]}}}}));
                continue;
            }
            "textDocument/inlayHint" => json!([{"position": {"line": 0, "character": 2}, "label": ": i32", "kind": 1}]),
            "textDocument/documentSymbol" => Value::Array(
                text.split('\n').enumerate().filter_map(|(i, l)| {
                    let at = l.find("fn ")?;
                    let name: String = l[at + 3..].chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
                    let start = utf16_col(l, at + 3);
                    Some(json!({"name": name, "kind": 12, "range": range(i as u32, 0, utf16_col(l, l.len())), "selectionRange": range(i as u32, start, start + name.len() as u32)}))
                }).collect(),
            ),
            "workspace/symbol" => {
                let query = params["query"].as_str().unwrap_or_default().to_string();
                let texts = state.texts.lock().unwrap().clone();
                let mut out = Vec::new();
                for (uri, text) in texts {
                    for (i, l) in text.split('\n').enumerate() {
                        if let Some(at) = l.find("fn ") {
                            let name: String = l[at + 3..].chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
                            if name.contains(&query) {
                                out.push(json!({"name": name, "kind": 12, "location": {"uri": uri, "range": range(i as u32, 0, 1)}}));
                            }
                        }
                    }
                }
                Value::Array(out)
            }
            "textDocument/semanticTokens/full" => {
                let mut data = Vec::new();
                let (mut last_line, mut last_start) = (0u32, 0u32);
                for (l, s, _) in occurrences(&text, "fn") {
                    let delta_line = l - last_line;
                    let delta_start = if delta_line == 0 { s - last_start } else { s };
                    data.extend([delta_line, delta_start, 2, 0, 0]);
                    (last_line, last_start) = (l, s);
                }
                json!({"data": data})
            }
            _ => {
                if let Some(id) = id {
                    send(&mut writer, json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": "unknown"}}));
                }
                continue;
            }
        };
        if let Some(id) = id {
            send(
                &mut writer,
                json!({"jsonrpc": "2.0", "id": id, "result": result}),
            );
        }
    }
}
