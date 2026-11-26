//! JSON-RPC 2.0 over a byte stream with LSP's `Content-Length` framing.
//!
//! A [`Connection`] owns two threads: a writer that drains an outgoing
//! queue (so a busy server never blocks the caller) and a reader that
//! matches responses to pending requests and hands notifications and
//! server-to-client requests to a [`Handler`]. Requests time out, and a
//! request future dropped before its answer sends `$/cancelRequest`.

use std::{
    collections::HashMap,
    io::{self, BufRead, BufReader, Read, Write},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicI64, Ordering},
        mpsc,
    },
    time::Duration,
};

use futures::{
    FutureExt as _,
    channel::oneshot,
    future::{Either, select},
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

/// Standard and LSP error codes.
pub mod codes {
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    pub const INTERNAL_ERROR: i64 = -32603;
    pub const SERVER_NOT_INITIALIZED: i64 = -32002;
    pub const REQUEST_CANCELLED: i64 = -32800;
    pub const CONTENT_MODIFIED: i64 = -32801;
}

#[derive(Debug, Clone, thiserror::Error, PartialEq)]
pub enum RpcError {
    #[error("the language server did not answer in time")]
    Timeout,
    #[error("the language server is not running")]
    Closed,
    #[error("{message} ({code})")]
    Response { code: i64, message: String },
    #[error("unexpected answer from the language server: {0}")]
    Decode(String),
}

impl RpcError {
    /// The request was overtaken by an edit or cancelled: not worth
    /// reporting, the next request will do.
    pub fn is_stale(&self) -> bool {
        matches!(
            self,
            RpcError::Response { code, .. }
                if *code == codes::CONTENT_MODIFIED || *code == codes::REQUEST_CANCELLED
        )
    }
}

/// A message from the server that is not a response.
#[derive(Debug, Clone)]
pub enum Incoming {
    Notification { method: String, params: Value },
    Request { method: String, params: Value },
}

/// Handles notifications and server-to-client requests on the reader
/// thread. For requests it returns the result (or an error code and
/// message); for notifications the return value is ignored.
pub type Handler = Arc<dyn Fn(Incoming) -> Result<Value, (i64, String)> + Send + Sync>;

/// Read one message; `Ok(None)` at the end of the stream.
pub fn read_message(reader: &mut impl BufRead) -> io::Result<Option<Value>> {
    let mut length: Option<usize> = None;
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        let header = line.trim_end();
        if header.is_empty() {
            if length.is_some() {
                break;
            }
            continue;
        }
        if let Some((name, value)) = header.split_once(':')
            && name.trim().eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse().ok();
        }
    }
    let mut body = vec![0; length.unwrap_or(0)];
    reader.read_exact(&mut body)?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

pub fn write_message(writer: &mut impl Write, message: &Value) -> io::Result<()> {
    let body = serde_json::to_vec(message)?;
    write!(writer, "Content-Length: {}\r\n\r\n", body.len())?;
    writer.write_all(&body)?;
    writer.flush()
}

type Pending = HashMap<i64, oneshot::Sender<Result<Value, RpcError>>>;

pub struct Connection {
    name: String,
    outgoing: Mutex<Option<mpsc::Sender<Value>>>,
    pending: Mutex<Pending>,
    next_id: AtomicI64,
    closed: AtomicBool,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Connection {
    /// Start the reader and writer threads. `on_close` runs on the reader
    /// thread once the stream ends (the server exited or closed stdout).
    pub fn start(
        name: &str,
        reader: Box<dyn Read + Send>,
        writer: Box<dyn Write + Send>,
        handler: Handler,
        on_close: Box<dyn FnOnce() + Send>,
    ) -> Arc<Self> {
        let (tx, rx) = mpsc::channel::<Value>();
        let conn = Arc::new(Self {
            name: name.to_string(),
            outgoing: Mutex::new(Some(tx)),
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicI64::new(1),
            closed: AtomicBool::new(false),
        });
        let writer_name = name.to_string();
        std::thread::Builder::new()
            .name(format!("lsp-write-{name}"))
            .spawn(move || {
                let mut writer = io::BufWriter::new(writer);
                while let Ok(message) = rx.recv() {
                    if let Err(err) = write_message(&mut writer, &message) {
                        tracing::debug!("{writer_name}: write failed: {err}");
                        break;
                    }
                }
            })
            .expect("spawn the LSP writer thread");
        let reader_conn = conn.clone();
        std::thread::Builder::new()
            .name(format!("lsp-read-{name}"))
            .spawn(move || {
                let mut reader = BufReader::new(reader);
                loop {
                    match read_message(&mut reader) {
                        Ok(Some(message)) => reader_conn.dispatch(message, &handler),
                        Ok(None) => break,
                        Err(err) => {
                            tracing::warn!("{}: {err}", reader_conn.name);
                            if err.kind() != io::ErrorKind::InvalidData {
                                break;
                            }
                        }
                    }
                }
                reader_conn.close();
                on_close();
            })
            .expect("spawn the LSP reader thread");
        conn
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    /// Fail every pending request and stop writing.
    pub fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        lock(&self.outgoing).take();
        lock(&self.pending).clear();
    }

    fn send(&self, message: Value) -> bool {
        match lock(&self.outgoing).as_ref() {
            Some(tx) => tx.send(message).is_ok(),
            None => false,
        }
    }

    fn dispatch(&self, message: Value, handler: &Handler) {
        let method = message.get("method").and_then(Value::as_str);
        let id = message.get("id").cloned();
        match (method, id) {
            (Some(method), Some(id)) => {
                let params = message.get("params").cloned().unwrap_or(Value::Null);
                let reply = match handler(Incoming::Request {
                    method: method.to_string(),
                    params,
                }) {
                    Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
                    Err((code, message)) => json!({
                        "jsonrpc": "2.0", "id": id,
                        "error": {"code": code, "message": message},
                    }),
                };
                self.send(reply);
            }
            (Some(method), None) => {
                let params = message.get("params").cloned().unwrap_or(Value::Null);
                let _ = handler(Incoming::Notification {
                    method: method.to_string(),
                    params,
                });
            }
            (None, Some(id)) => {
                let Some(id) = id.as_i64() else { return };
                let Some(tx) = lock(&self.pending).remove(&id) else {
                    return;
                };
                let result = if let Some(error) = message.get("error") {
                    Err(RpcError::Response {
                        code: error.get("code").and_then(Value::as_i64).unwrap_or(0),
                        message: error
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    })
                } else {
                    Ok(message.get("result").cloned().unwrap_or(Value::Null))
                };
                let _ = tx.send(result);
            }
            (None, None) => {}
        }
    }

    pub fn notify(&self, method: &str, params: impl Serialize) {
        let params = serde_json::to_value(params).unwrap_or(Value::Null);
        self.send(json!({"jsonrpc": "2.0", "method": method, "params": params}));
    }

    /// Send a request; the answer as JSON. Dropping the future before it
    /// completes cancels the request on the server.
    pub fn request_value(
        self: &Arc<Self>,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> impl Future<Output = Result<Value, RpcError>> + Send + 'static {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        let sent = if self.is_closed() {
            false
        } else {
            lock(&self.pending).insert(id, tx);
            self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
        };
        let guard = CancelGuard {
            conn: Arc::downgrade(self),
            id,
            armed: sent,
        };
        async move {
            // Move the whole guard into the future (a field capture would
            // leave it behind and cancel the request at once).
            let mut guard = guard;
            if !sent {
                guard.armed = false;
                return Err(RpcError::Closed);
            }
            let timer = smol::Timer::after(timeout);
            let result = match select(rx, timer.fuse()).await {
                Either::Left((Ok(result), _)) => result,
                Either::Left((Err(_), _)) => Err(RpcError::Closed),
                Either::Right(_) => return Err(RpcError::Timeout),
            };
            guard.armed = false;
            result
        }
    }

    /// [`Connection::request_value`] with typed params and result.
    pub fn request<R>(
        self: &Arc<Self>,
        params: R::Params,
        timeout: Duration,
    ) -> impl Future<Output = Result<R::Result, RpcError>> + Send + 'static
    where
        R: lsp_types::request::Request,
        R::Result: DeserializeOwned + Send + 'static,
    {
        let params = serde_json::to_value(params).unwrap_or(Value::Null);
        let request = self.request_value(R::METHOD, params, timeout);
        async move {
            let value = request.await?;
            serde_json::from_value(value).map_err(|e| RpcError::Decode(e.to_string()))
        }
    }
}

/// Sends `$/cancelRequest` when a request future is dropped unanswered
/// (the editor moved on: a newer completion, a closed popup) or timed out.
struct CancelGuard {
    conn: std::sync::Weak<Connection>,
    id: i64,
    armed: bool,
}

impl Drop for CancelGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        if let Some(conn) = self.conn.upgrade() {
            lock(&conn.pending).remove(&self.id);
            conn.notify("$/cancelRequest", json!({ "id": self.id }));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn framing_round_trips_and_skips_unknown_headers() {
        let mut buf = Vec::new();
        write_message(&mut buf, &json!({"a": "zażółć"})).unwrap();
        let mut input = b"Content-Type: application/vscode-jsonrpc; charset=utf-8\r\n".to_vec();
        input.extend_from_slice(&buf);
        input.extend_from_slice(&buf);
        let mut reader = io::Cursor::new(input);
        assert_eq!(
            read_message(&mut reader).unwrap(),
            Some(json!({"a": "zażółć"}))
        );
        assert_eq!(
            read_message(&mut reader).unwrap(),
            Some(json!({"a": "zażółć"}))
        );
        assert_eq!(read_message(&mut reader).unwrap(), None);
    }
}
