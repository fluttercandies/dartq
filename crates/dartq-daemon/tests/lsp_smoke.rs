//! End-to-end LSP session tests over an in-memory pipe: a real engine on a
//! throwaway workspace, a scripted client feeding framed messages through a
//! channel, the transcript asserted frame by frame. No real process spawns.

mod common;

use std::io::{BufRead, Read, Write};
use std::sync::{Arc, Mutex};

use common::{BROKEN_MAIN, CLEAN_MAIN, Workspace, frames};
use dartq_daemon::{Session, run};
use serde_json::{Value, json};
use std::sync::mpsc;

/// The client end of the transport: a channel of byte chunks read as a
/// stream. Disconnection is EOF.
struct ChannelReader {
    receiver: mpsc::Receiver<Vec<u8>>,
    pending: Vec<u8>,
}

impl Read for ChannelReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.pending.is_empty() {
            self.pending = self.receiver.recv().map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "client disconnected")
            })?;
        }
        let take = buf.len().min(self.pending.len());
        buf[..take].copy_from_slice(&self.pending[..take]);
        self.pending.drain(..take);
        Ok(take)
    }
}

impl BufRead for ChannelReader {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        if self.pending.is_empty() {
            self.pending = self.receiver.recv().map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "client disconnected")
            })?;
        }
        Ok(&self.pending)
    }

    fn consume(&mut self, amount: usize) {
        self.pending.drain(..amount);
    }
}

/// The server's output side: everything the session writes, appended.
#[derive(Clone, Default)]
struct SharedOutput(Arc<Mutex<Vec<u8>>>);

impl Write for SharedOutput {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("output lock").extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn framed(value: &Value) -> Vec<u8> {
    let body = serde_json::to_string(value).expect("serializes");
    format!("Content-Length: {}\r\n\r\n{body}", body.len()).into_bytes()
}

fn request(id: i64, method: &str, params: impl Into<Value>) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params.into()})
}

fn notification(method: &str, params: impl Into<Value>) -> Value {
    json!({"jsonrpc": "2.0", "method": method, "params": params.into()})
}

/// Runs the whole scripted session and returns (exit code, decoded frames).
fn drive(workspace: &Workspace, client: &[Value]) -> (u8, Vec<Value>) {
    let (sender, receiver) = mpsc::channel();
    for message in client {
        sender.send(framed(message)).expect("client send");
    }
    drop(sender); // client hangs up after `exit`

    let output = SharedOutput::default();
    let session = Session::new(workspace.engine());
    let reader = ChannelReader {
        receiver,
        pending: Vec::new(),
    };
    let code = run(std::io::BufReader::new(reader), output.clone(), session).expect("session");
    (code, frames(&output.0.lock().expect("output lock")))
}

fn responses(frames: &[Value]) -> Vec<&Value> {
    frames
        .iter()
        .filter(|frame| frame.get("id").is_some())
        .collect()
}

fn publishes(frames: &[Value]) -> Vec<&Value> {
    frames
        .iter()
        .filter(|frame| frame["method"] == "textDocument/publishDiagnostics")
        .collect()
}

#[test]
fn full_session_overlay_and_diagnostics() {
    let workspace = Workspace::new(BROKEN_MAIN);
    let uri = dartq_daemon::uri::from_path(&workspace.path("lib/main.dart"));

    let (code, transcript) = drive(
        &workspace,
        &[
            request(1, "initialize", json!({"processId": null, "rootUri": null})),
            notification("initialized", json!({})),
            notification(
                "textDocument/didOpen",
                json!({"textDocument": {"uri": uri, "languageId": "dart", "version": 1,
                       "text": BROKEN_MAIN}}),
            ),
            notification(
                "textDocument/didChange",
                json!({"textDocument": {"uri": uri, "version": 2},
                       "contentChanges": [{"text": CLEAN_MAIN}]}),
            ),
            request(2, "dartq/analyze", json!({})),
            notification(
                "textDocument/didClose",
                json!({"textDocument": {"uri": uri}}),
            ),
            request(3, "shutdown", json!(null)),
            notification("exit", json!(null)),
        ],
    );

    assert_eq!(code, dartq_daemon::EXIT_CLEAN, "shutdown+exit exits 0");

    // initialize answered with the negotiated capabilities.
    let answered = responses(&transcript);
    assert_eq!(answered[0]["id"], 1);
    assert_eq!(answered[0]["result"]["capabilities"]["textDocumentSync"], 1);

    // The `initialized` sweep publishes the seeded disk error; didOpen with
    // identical text must NOT re-publish (the diff baseline holds).
    let pushes = publishes(&transcript);
    assert_eq!(
        pushes.len(),
        3,
        "exactly: sweep push, didChange clear push, didClose restore push"
    );
    assert_eq!(pushes[0]["params"]["uri"], uri);
    assert_eq!(pushes[0]["params"]["version"], Value::Null);
    let initial = &pushes[0]["params"]["diagnostics"];
    assert!(
        initial
            .as_array()
            .expect("array")
            .iter()
            .any(|d| d["code"] == "uri_does_not_exist"),
        "sweep reports the seeded error: {initial}"
    );

    // The didChange publish cleared the diagnostics under the client's
    // version, and the dartq/analyze answer (which runs after the change)
    // agrees.
    assert_eq!(pushes[1]["params"]["uri"], uri);
    assert_eq!(pushes[1]["params"]["version"], 2);
    assert_eq!(pushes[1]["params"]["diagnostics"], json!([]));
    let analyze = answered
        .iter()
        .find(|frame| frame["id"] == 2)
        .expect("analyze answer");
    assert_eq!(analyze["result"]["diagnostics"], json!([]));

    // didClose restored the disk content: the error is back, unversioned.
    assert_eq!(pushes[2]["params"]["uri"], uri);
    assert_eq!(pushes[2]["params"]["version"], Value::Null);
    let restored = &pushes[2]["params"]["diagnostics"];
    assert!(
        restored
            .as_array()
            .expect("array")
            .iter()
            .any(|d| d["code"] == "uri_does_not_exist"),
        "didClose falls back to disk state: {restored}"
    );

    // shutdown answers null.
    let shutdown = answered
        .iter()
        .find(|frame| frame["id"] == 3)
        .expect("shutdown answer");
    assert_eq!(shutdown["result"], Value::Null);
    assert_eq!(shutdown.get("error"), None);
}

#[test]
fn lifecycle_gates_and_exit_without_shutdown() {
    let workspace = Workspace::new(CLEAN_MAIN);

    let (code, transcript) = drive(
        &workspace,
        &[
            // Any request before initialize is ServerNotInitialized.
            request(1, "shutdown", json!(null)),
            notification(
                "textDocument/didOpen",
                json!({"textDocument": {
                    "uri": "file:///x.dart", "languageId": "dart", "version": 1, "text": ""
                }}),
            ),
            // Still uninitialized: unknown methods and notifications behave.
            notification("some/unknown", json!({})),
            request(2, "initialize", json!({})),
            request(3, "no/such/method", json!({})),
            // exit without shutdown: the process exits 1 (LSP contract).
            notification("exit", json!(null)),
        ],
    );

    assert_eq!(code, dartq_daemon::EXIT_WITHOUT_SHUTDOWN);
    let answered = responses(&transcript);
    assert_eq!(
        answered.len(),
        3,
        "three requests, three answers: {answered:?}"
    );
    assert_eq!(answered[0]["error"]["code"], -32002);
    assert_eq!(answered[1]["id"], 2);
    assert_eq!(answered[1]["result"]["capabilities"]["textDocumentSync"], 1);
    assert_eq!(answered[2]["error"]["code"], -32601);
    // The dropped didOpen/unknown notifications produced no pushes.
    assert!(publishes(&transcript).is_empty());
}

#[test]
fn unparsable_body_answers_and_session_continues() {
    let workspace = Workspace::new(CLEAN_MAIN);

    let (sender, receiver) = mpsc::channel();
    sender
        .send(b"Content-Length: 8\r\n\r\nnot json".to_vec())
        .expect("send");
    sender
        .send(framed(&request(1, "initialize", json!({}))))
        .expect("send");
    sender
        .send(framed(&request(2, "shutdown", json!(null))))
        .expect("send");
    sender
        .send(framed(&notification("exit", json!(null))))
        .expect("send");
    drop(sender);

    let output = SharedOutput::default();
    let session = Session::new(workspace.engine());
    let reader = ChannelReader {
        receiver,
        pending: Vec::new(),
    };
    let code = run(std::io::BufReader::new(reader), output.clone(), session).expect("session");

    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
    let transcript = frames(&output.0.lock().expect("output lock"));
    let answered = responses(&transcript);
    let parse_error = &answered[0];
    assert_eq!(
        parse_error["error"]["code"], -32700,
        "parse error answered: {parse_error}"
    );
    assert_eq!(parse_error["id"], Value::Null);
    assert_eq!(answered[1]["result"]["capabilities"]["textDocumentSync"], 1);
    assert_eq!(answered[2]["result"], Value::Null, "shutdown answers null");
}

#[test]
fn broken_framing_ends_the_session() {
    let workspace = Workspace::new(CLEAN_MAIN);

    let (sender, receiver) = mpsc::channel();
    // No Content-Length: the stream cannot be resynchronized.
    sender
        .send(b"X-Drop: it\r\n\r\norphan body".to_vec())
        .expect("send");
    // The server must answer ParseError and stop reading — this frame is
    // never processed.
    sender
        .send(framed(&request(1, "initialize", json!({}))))
        .expect("send");
    drop(sender);

    let output = SharedOutput::default();
    let session = Session::new(workspace.engine());
    let reader = ChannelReader {
        receiver,
        pending: Vec::new(),
    };
    let code = run(std::io::BufReader::new(reader), output.clone(), session).expect("session");

    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
    let transcript = frames(&output.0.lock().expect("output lock"));
    assert_eq!(transcript.len(), 1, "one answer, then the session ends");
    assert_eq!(transcript[0]["error"]["code"], -32700);
}
