//! The server: reader thread → channel → main loop. The reader thread only
//! decodes frames; the main loop (the caller's thread) owns the output, the
//! lifecycle state machine, and the [`Session`], so neither the engine nor
//! the writer ever needs a lock.
//!
//! # Turn shape
//!
//! One turn = block on the next frame, then drain everything already queued
//! into one batch (the rust-analyzer main-loop posture): N overlay edits and
//! M requests staged by the batch execute at a single
//! [`Session::service`] drain, in the scheduler's writer-priority order —
//! every write applies before any read analyzes. The snapshot debounce is
//! checked at the same turn boundary: its window elapses across the blocking
//! receive, so the loop stays timer-free.

use std::io::{self, BufRead, Write};
use std::sync::mpsc;
use std::thread;
use std::time::Instant;

use serde_json::{Value, json};

use crate::codec::{self, FrameError};
use crate::jsonrpc::{self, Message, Notification, Request, Response, codes};
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
    while let Ok(first) = receiver.recv() {
        // Turn start: the snapshot debounce window elapses across the
        // blocking receive above, so the flush check happens before intake.
        // (Only tests read the attempt outcome.)
        let _ = session.flush_if_due(Instant::now());

        // The batch window: every frame already queued joins this turn's
        // single drain (PRD §16.5 writer priority).
        let mut batch = vec![first];
        while let Ok(incoming) = receiver.try_recv() {
            batch.push(incoming);
        }

        let mut end = false;
        for incoming in batch {
            match incoming {
                Incoming::Message(Message::Request(request)) => {
                    if let Some(response) = handle_request(&mut lifecycle, &mut session, request) {
                        write_message(&mut output, &Message::Response(response))?;
                    }
                }
                Incoming::Message(Message::Notification(notification)) => {
                    if let Some(code) =
                        handle_notification(&mut lifecycle, &mut session, &notification)
                    {
                        exit_code = Some(code);
                        end = true;
                        break;
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
                    end = true;
                    break;
                }
                Incoming::Eof => {
                    end = true;
                    break;
                }
            }
        }
        if end {
            break;
        }

        // The drain: staged writes apply, the coalesced read analyzes, and
        // stale (cancelled) requests answer without a result.
        let turn = session.service();
        for publish in turn.publishes {
            send_publish(&mut output, publish)?;
        }
        for (id, answer) in turn.answers {
            let response = match answer {
                Ok(entries) => Response::ok(id, json!({ "diagnostics": entries })),
                Err(error) => answer_error(id, &error),
            };
            write_message(&mut output, &Message::Response(response))?;
        }
    }

    // #306: the clean-shutdown write, window or no window.
    session.flush_snapshot();

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

/// Answers a request, or `None` when the request queued onto the session's
/// scheduler and is answered by the turn's drain instead.
fn handle_request(
    lifecycle: &mut Lifecycle,
    session: &mut Session,
    request: Request,
) -> Option<Response> {
    let id = request.id;
    match (*lifecycle, request.method.as_str()) {
        (Lifecycle::Uninitialized, "initialize") => {
            *lifecycle = Lifecycle::Ready;
            Some(initialize_response(id))
        }
        // A second `initialize` is a client bug (LSP: initialize must be the
        // first request).
        (Lifecycle::Ready, "initialize") => Some(Response::error(
            Some(id),
            codes::INVALID_REQUEST,
            "server is already initialized",
        )),
        (Lifecycle::Ready, "shutdown") => {
            *lifecycle = Lifecycle::ShutDown;
            Some(Response::ok(id, Value::Null))
        }
        (Lifecycle::Ready, "dartq/analyze") => {
            session.queue_analyze(id);
            None
        }
        (Lifecycle::Uninitialized, _) => Some(Response::error(
            Some(id),
            codes::SERVER_NOT_INITIALIZED,
            "server is not initialized",
        )),
        (Lifecycle::Ready, method) => Some(Response::error(
            Some(id),
            codes::METHOD_NOT_FOUND,
            format!("method not found: {method}"),
        )),
        // Requests during shutdown are rejected above the state machine
        // (LSP §lifeCycleMessages); nothing else lands here.
        (Lifecycle::ShutDown, _) => Some(Response::error(
            Some(id),
            codes::INVALID_REQUEST,
            "server is shutting down",
        )),
    }
}

/// Answers `initialize` with the negotiated capability set.
fn initialize_response(id: jsonrpc::RequestId) -> Response {
    Response::ok(
        id,
        json!({
            "capabilities": {
                "positionEncoding": "utf-16",
                "textDocumentSync": 1,
                // Non-LSP request surface: the raw-diagnose `dartq/analyze`.
                // Announced as experimental so clients can opt in by name.
                "experimental": {
                    "dartq/analyze": {},
                },
            },
            "serverInfo": {
                "name": "dartq",
                "version": env!("CARGO_PKG_VERSION"),
            },
        }),
    )
}

/// Handles one notification; `Some` is the `exit` process code.
fn handle_notification(
    lifecycle: &mut Lifecycle,
    session: &mut Session,
    notification: &Notification,
) -> Option<u8> {
    if notification.method == "exit" {
        return Some(if *lifecycle == Lifecycle::ShutDown {
            EXIT_CLEAN
        } else {
            EXIT_WITHOUT_SHUTDOWN
        });
    }
    // Notifications before `initialize` and after `shutdown` are dropped
    // (LSP: only `exit` is honored in either state).
    if *lifecycle != Lifecycle::Ready {
        return None;
    }
    let params = notification.params.clone().unwrap_or(Value::Null);
    match notification.method.as_str() {
        "initialized" => {
            if let Err(error) = session.sweep() {
                failed(&notification.method, &error);
            }
        }
        "textDocument/didOpen" => {
            if let Ok(open) = serde_json::from_value::<lsp::DidOpenParams>(params) {
                let document = open.text_document;
                session.queue_edit(&document.uri, &document.text, document.version);
            } else {
                eprintln!("dartq-daemon: dropping malformed didOpen");
            }
        }
        "textDocument/didChange" => match serde_json::from_value::<lsp::DidChangeParams>(params) {
            // Full sync: events arrive in order, each carrying the whole
            // document; the last well-formed one wins. A `range` marks a
            // full-sync violation and cannot be applied.
            Ok(change) => match change
                .content_changes
                .into_iter()
                .rev()
                .find(|event| event.range.is_none())
            {
                Some(event) => {
                    session.queue_edit(
                        &change.text_document.uri,
                        &event.text,
                        change.text_document.version,
                    );
                }
                None => eprintln!(
                    "dartq-daemon: dropping didChange without a full-text event (ranged edits \
                     need incremental sync)"
                ),
            },
            Err(error) => eprintln!("dartq-daemon: dropping malformed didChange: {error}"),
        },
        "textDocument/didClose" => match serde_json::from_value::<lsp::DidCloseParams>(params) {
            Ok(close) => {
                if let Err(error) = session.queue_close(&close.text_document.uri) {
                    failed(&notification.method, &error.into());
                }
            }
            Err(error) => eprintln!("dartq-daemon: dropping malformed didClose: {error}"),
        },
        "$/cancelRequest" => match params.get("id").and_then(jsonrpc::RequestId::from_value) {
            Some(id) => session.cancel_request(&id),
            None => eprintln!("dartq-daemon: dropping cancelRequest without an id"),
        },
        // Unknown notifications are ignored (LSP §messageHandling).
        _ => {}
    }
    None
}

/// Maps a turn answer's failure to its response: a stale (cancelled) read
/// answers LSP's `RequestCancelled`; anything else is surfaced as
/// `InternalError`.
fn answer_error(id: jsonrpc::RequestId, error: &SessionError) -> Response {
    match error {
        SessionError::Cancelled => Response::error(
            Some(id),
            codes::REQUEST_CANCELLED,
            "request cancelled before execution",
        ),
        SessionError::Io(io) => Response::error(Some(id), codes::INTERNAL_ERROR, io.to_string()),
    }
}

/// Notes a failed session call for an unanswerable notification.
fn failed(method: &str, error: &SessionError) {
    eprintln!("dartq-daemon: {method} failed: {error}");
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
        assert!(
            result["capabilities"]["experimental"]
                .get("dartq/analyze")
                .is_some(),
            "the experimental analyze request is announced"
        );
        assert_eq!(result["serverInfo"]["name"], "dartq");
    }
}
