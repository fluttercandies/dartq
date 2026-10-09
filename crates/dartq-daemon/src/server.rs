//! The server: reader thread → channel → main loop. The reader thread only
//! decodes frames; the main loop (the caller's thread) owns the output, the
//! lifecycle state machine, and the [`Session`], so neither the engine nor
//! the writer ever needs a lock.

use std::io::{self, BufRead, Write};
use std::sync::mpsc;
use std::thread;

use serde_json::{Value, json};

use crate::codec::{self, FrameError};
use crate::jsonrpc::{Message, Notification, Request, Response, codes};
use crate::lsp::{self, PublishDiagnosticsParams};
use crate::session::{Publish, Session, SessionError};

/// Exit code for a clean session end: `shutdown` + `exit`, or the client
/// simply closed the stream.
pub const EXIT_CLEAN: u8 = 0;
/// Exit code for `exit` without a prior `shutdown` (LSP §lifeCycleMessages).
pub const EXIT_WITHOUT_SHUTDOWN: u8 = 1;

/// What the reader thread hands to the main loop.
enum Incoming {
    Message(Message),
    /// A well-framed body that is not valid JSON-RPC: answerable, and the
    /// stream stays in sync.
    Unparsable {
        code: i64,
        message: String,
    },
    /// The framing broke beyond resynchronization: one best-effort
    /// `ParseError` answer, then the session ends.
    Desynced(String),
    /// Clean end of stream.
    Eof,
}

/// What one notification leaves the main loop to do.
enum Outcome {
    /// `exit` — end the session with this process exit code.
    Exit(u8),
    /// Push these diagnostics.
    Publish(Vec<Publish>),
    /// Nothing to do.
    Nothing,
}

/// Runs one LSP session to its end and returns the process exit code.
///
/// Spawns the reader thread and runs the main loop on the current thread.
/// Returns after `exit` (possibly leaving the reader blocked on the next
/// read — under a real process the caller exits immediately, which reaps
/// it; the reader is joined only when it is known to have finished) or when
/// the stream ends.
///
/// # Errors
///
/// Writing to the client failed (peer vanished mid-answer).
pub fn run<R, W>(input: R, output: W, mut session: Session) -> io::Result<u8>
where
    R: BufRead + Send + 'static,
    W: Write + 'static,
{
    let (sender, receiver) = mpsc::channel::<Incoming>();
    let reader = thread::Builder::new()
        .name("dartq-daemon-reader".into())
        .spawn(move || reader_loop(input, sender))?;

    let mut output = output;
    let mut lifecycle = Lifecycle::Uninitialized;
    // Set only by `exit`; every other break falls through to `EXIT_CLEAN`.
    let mut exit_code: Option<u8> = None;

    // Ends when the reader thread disconnects or a notification breaks out.
    while let Ok(incoming) = receiver.recv() {
        match incoming {
            Incoming::Message(Message::Request(request)) => {
                let response = handle_request(&mut lifecycle, &mut session, request);
                write_message(&mut output, &Message::Response(response))?;
            }
            Incoming::Message(Message::Notification(notification)) => {
                match handle_notification(&mut lifecycle, &mut session, &notification) {
                    Outcome::Exit(code) => {
                        exit_code = Some(code);
                        break;
                    }
                    Outcome::Publish(publishes) => {
                        for publish in publishes {
                            send_publish(&mut output, publish)?;
                        }
                    }
                    Outcome::Nothing => {}
                }
            }
            Incoming::Message(Message::Response(_)) => {} // clients never send these
            Incoming::Unparsable { code, message } => {
                write_message(
                    &mut output,
                    &Message::Response(Response::error(None, code, message)),
                )?;
            }
            Incoming::Desynced(message) => {
                eprintln!("dartq-daemon: {message}; ending session");
                write_message(
                    &mut output,
                    &Message::Response(Response::error(None, codes::PARSE_ERROR, message)),
                )?;
                break;
            }
            Incoming::Eof => break,
        }
    }

    if reader.is_finished() {
        let _ = reader.join();
    }
    Ok(exit_code.unwrap_or(EXIT_CLEAN))
}

/// Reads frames until the stream or the channel ends. Takes the sender by
/// value: it is the thread's single owner, even though `send` borrows.
#[allow(clippy::needless_pass_by_value)]
fn reader_loop(input: impl BufRead, sender: mpsc::Sender<Incoming>) {
    let mut input = input;
    loop {
        match codec::read_frame(&mut input) {
            Ok(Some(body)) => {
                let incoming = match serde_json::from_slice::<Value>(&body) {
                    Ok(value) => match Message::from_value(value) {
                        Ok(message) => Incoming::Message(message),
                        Err(message) => Incoming::Unparsable {
                            code: codes::INVALID_REQUEST,
                            message,
                        },
                    },
                    Err(error) => Incoming::Unparsable {
                        code: codes::PARSE_ERROR,
                        message: error.to_string(),
                    },
                };
                if sender.send(incoming).is_err() {
                    return;
                }
            }
            Ok(None) => {
                let _ = sender.send(Incoming::Eof);
                return;
            }
            Err(FrameError::Io(error)) => {
                eprintln!("dartq-daemon: input stream failed: {error}");
                let _ = sender.send(Incoming::Eof);
                return;
            }
            Err(FrameError::Protocol(message)) => {
                let _ = sender.send(Incoming::Desynced(message));
                return;
            }
        }
    }
}

/// The LSP lifecycle: `initialize` gates everything, `shutdown` closes the
/// request surface, `exit` ends the process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lifecycle {
    Uninitialized,
    Ready,
    ShutDown,
}

fn handle_request(lifecycle: &mut Lifecycle, session: &mut Session, request: Request) -> Response {
    let id = request.id;
    match (*lifecycle, request.method.as_str()) {
        (Lifecycle::Uninitialized, "initialize") => {
            *lifecycle = Lifecycle::Ready;
            initialize_response(id)
        }
        // A second `initialize` is a client bug (LSP: initialize must be the
        // first request).
        (Lifecycle::Ready, "initialize") => Response::error(
            Some(id),
            codes::INVALID_REQUEST,
            "server is already initialized",
        ),
        (Lifecycle::Ready, "shutdown") => {
            *lifecycle = Lifecycle::ShutDown;
            Response::ok(id, Value::Null)
        }
        (Lifecycle::Ready, "dartq/analyze") => match session.analyze_report() {
            Ok(entries) => Response::ok(id, json!({ "diagnostics": entries })),
            Err(error) => Response::error(Some(id), codes::INTERNAL_ERROR, error.to_string()),
        },
        (Lifecycle::Uninitialized, _) => Response::error(
            Some(id),
            codes::SERVER_NOT_INITIALIZED,
            "server is not initialized",
        ),
        (Lifecycle::Ready, method) => Response::error(
            Some(id),
            codes::METHOD_NOT_FOUND,
            format!("method not found: {method}"),
        ),
        // Requests during shutdown are rejected above the state machine
        // (LSP §lifeCycleMessages); nothing else lands here.
        (Lifecycle::ShutDown, _) => {
            Response::error(Some(id), codes::INVALID_REQUEST, "server is shutting down")
        }
    }
}

/// Answers `initialize` with the negotiated capability set.
fn initialize_response(id: crate::jsonrpc::RequestId) -> Response {
    Response::ok(
        id,
        json!({
            "capabilities": {
                "positionEncoding": "utf-16",
                "textDocumentSync": 1,
            },
            "serverInfo": {
                "name": "dartq",
                "version": env!("CARGO_PKG_VERSION"),
            },
        }),
    )
}

fn handle_notification(
    lifecycle: &mut Lifecycle,
    session: &mut Session,
    notification: &Notification,
) -> Outcome {
    if notification.method == "exit" {
        return Outcome::Exit(if *lifecycle == Lifecycle::ShutDown {
            EXIT_CLEAN
        } else {
            EXIT_WITHOUT_SHUTDOWN
        });
    }
    // Notifications before `initialize` and after `shutdown` are dropped
    // (LSP: only `exit` is honored in either state).
    if *lifecycle != Lifecycle::Ready {
        return Outcome::Nothing;
    }
    let params = notification.params.clone().unwrap_or(Value::Null);
    match notification.method.as_str() {
        "initialized" => match session.sweep() {
            Ok(publishes) => Outcome::Publish(publishes),
            Err(error) => failed(&notification.method, &error),
        },
        "textDocument/didOpen" => match serde_json::from_value::<lsp::DidOpenParams>(params) {
            Ok(open) => {
                let document = open.text_document;
                match session.open(&document.uri, &document.text, document.version) {
                    Ok(Some(publishes)) => Outcome::Publish(publishes),
                    Ok(None) => Outcome::Nothing,
                    Err(error) => failed(&notification.method, &error),
                }
            }
            Err(error) => {
                eprintln!("dartq-daemon: dropping malformed didOpen: {error}");
                Outcome::Nothing
            }
        },
        "textDocument/didChange" => match serde_json::from_value::<lsp::DidChangeParams>(params) {
            Ok(change) => {
                // Full sync: events arrive in order, each carrying the whole
                // document; the last well-formed one wins. A `range` marks a
                // full-sync violation and cannot be applied.
                let Some(text) = change
                    .content_changes
                    .into_iter()
                    .rev()
                    .find(|event| event.range.is_none())
                    .map(|event| event.text)
                else {
                    eprintln!(
                        "dartq-daemon: dropping didChange without a full-text event (ranged \
                         edits need incremental sync)"
                    );
                    return Outcome::Nothing;
                };
                let uri = change.text_document.uri;
                let version = change.text_document.version;
                match session.change(&uri, &text, version) {
                    Ok(Some(publishes)) => Outcome::Publish(publishes),
                    Ok(None) => Outcome::Nothing,
                    Err(error) => failed(&notification.method, &error),
                }
            }
            Err(error) => {
                eprintln!("dartq-daemon: dropping malformed didChange: {error}");
                Outcome::Nothing
            }
        },
        "textDocument/didClose" => match serde_json::from_value::<lsp::DidCloseParams>(params) {
            Ok(close) => match session.close(&close.text_document.uri) {
                Ok(Some(publishes)) => Outcome::Publish(publishes),
                Ok(None) => Outcome::Nothing,
                Err(error) => failed(&notification.method, &error),
            },
            Err(error) => {
                eprintln!("dartq-daemon: dropping malformed didClose: {error}");
                Outcome::Nothing
            }
        },
        // Unknown notifications are ignored (LSP §messageHandling).
        _ => Outcome::Nothing,
    }
}

/// Notes a failed analysis for an unanswerable notification.
fn failed(method: &str, error: &SessionError) -> Outcome {
    eprintln!("dartq-daemon: {method} analysis failed: {error}");
    Outcome::Nothing
}

fn send_publish(output: &mut impl Write, publish: Publish) -> io::Result<()> {
    let params = PublishDiagnosticsParams {
        uri: publish.uri,
        version: publish.version,
        diagnostics: publish.diagnostics,
    };
    write_message(
        output,
        &Message::Notification(Notification::new(
            "textDocument/publishDiagnostics",
            serde_json::to_value(params).expect("publish params serialize"),
        )),
    )
}

fn write_message(output: &mut impl Write, message: &Message) -> io::Result<()> {
    let body = serde_json::to_vec(&message.to_value()).expect("messages serialize");
    codec::write_frame(output, &body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jsonrpc::RequestId;

    #[test]
    fn initialize_capabilities_are_pinned() {
        let Response { result, error, id } = initialize_response(RequestId::Num(1));
        let result = result.expect("capabilities result");
        assert_eq!(error, None);
        assert_eq!(id, Some(RequestId::Num(1)));
        assert_eq!(result["capabilities"]["textDocumentSync"], 1);
        assert_eq!(result["capabilities"]["positionEncoding"], "utf-16");
        assert_eq!(result["serverInfo"]["name"], "dartq");
    }
}
