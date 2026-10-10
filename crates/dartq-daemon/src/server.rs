//! The server: reader thread → channel → main loop. The reader thread only
//! decodes frames; the main loop (the caller's thread) owns the output, the
//! lifecycle state machine, and the [`Session`], so neither the engine nor
//! the writer ever needs a lock.
//!
//! # Turn shape
//!
//! One turn = block on the next frames, then drain everything already queued
//! into one batch (the rust-analyzer main-loop posture): the reader forwards
//! each buffered chunk as one item, so frames the client wrote together are
//! one turn's intake. N overlay edits and M requests staged by the batch
//! execute at a single [`Session::service`] drain, in the scheduler's
//! writer-priority order — every write applies before any read analyzes. The
//! snapshot debounce is checked at the same turn boundary; while a save is
//! pending the receive is bounded by the window's remainder, so an idle
//! daemon wakes at the deadline to save — still no timer thread.

use std::io::{self, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::Instant;

use serde::Deserialize;
use serde_json::{Value, json};

use dartq_commands::Verbosity;
use dartq_commands::fix::{self, PluginHosts};

use crate::codec::{self, FrameError};
use crate::commands;
use crate::jsonrpc::{self, Message, Notification, Request, Response, codes};
use crate::lsp::{self, PublishDiagnosticsParams};
use crate::session::{Publish, Session, SessionError};
use crate::watch::{WatchOptions, Watcher, is_config_input};

/// Exit code for a clean session end: `shutdown` + `exit`, or the client
/// simply closed the stream.
pub const EXIT_CLEAN: u8 = 0;
/// Exit code for `exit` without a prior `shutdown` (LSP §lifeCycleMessages).
pub const EXIT_WITHOUT_SHUTDOWN: u8 = 1;

/// What the reader thread hands to the main loop — plus the watch thread's
/// debounced batches, which ride the same channel (one receiver, no select).
pub(crate) enum Incoming {
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
    /// `--watch` intake (PRD §16.7 mode 3): the debounced batch of paths
    /// whose on-disk content changed or vanished since the last batch —
    /// synced didChange-equivalent at the turn's drain.
    Watch {
        touched: Vec<PathBuf>,
    },
}

/// Runs one LSP session to its end and returns the process exit code.
///
/// Spawns the reader thread and runs the main loop on the current thread.
/// With `watch` set (PRD §16.7 mode 3) it also spawns the one watch thread,
/// whose debounced batches ride the same channel; dropping the watcher at
/// session end stops and joins it. Returns after `exit` (possibly leaving
/// the reader blocked on the next read — under a real process the caller
/// exits immediately, which reaps it; the reader is joined only when it is
/// known to have finished) or when the stream ends. Input buffering is the
/// shell's policy: the reader thread wraps the stream so batch boundaries
/// follow read granularity.
///
/// # Errors
///
/// Writing to the client failed (peer vanished mid-answer).
pub fn run<R, W>(
    input: R,
    output: W,
    mut session: Session,
    watch: Option<WatchOptions>,
) -> io::Result<u8>
where
    R: Read + Send + 'static,
    W: Write + 'static,
{
    let (sender, receiver) = mpsc::channel::<Vec<Incoming>>();
    // Held for the session's lifetime: dropping it at return stops and
    // joins the watch thread. A send past a dead receiver ends the thread
    // on its own, too.
    let _watcher = watch.map(|options| Watcher::spawn(options, sender.clone()));
    let reader = thread::Builder::new()
        .name("dartq-daemon-reader".into())
        .spawn(move || reader_loop(BufReader::new(input), sender))?;

    let mut output = output;
    let mut lifecycle = Lifecycle::Uninitialized;
    // Set only by `exit`; every other break falls through to `EXIT_CLEAN`.
    let mut exit_code: Option<u8> = None;
    // The plugin-dispatch faces' host set: launched at the first
    // `dartq/fix`/`dartq/assists` request, shut down when the session ends.
    let mut fix_hosts = FixHosts::new(session.workspace_root());

    // Ends when the reader thread disconnects or a notification breaks out.
    loop {
        // Turn start: the snapshot debounce window elapses across the
        // receive below — blocking or deadline-bounded — so the flush check
        // happens before intake. (Only tests read the attempt outcome.)
        let _ = session.flush_if_due(Instant::now());

        // While a save is pending, the receive is bounded by the window's
        // remainder: an idle daemon wakes at the deadline and the turn-start
        // check above saves, instead of blocking forever with the save
        // queued. A clean sink blocks freely until the next frame. Either
        // way the loop stays timer-free.
        let first = match session.flush_deadline() {
            None => match receiver.recv() {
                Ok(first) => first,
                Err(_) => break,
            },
            Some(deadline) => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                match receiver.recv_timeout(remaining) {
                    Ok(first) => first,
                    Err(RecvTimeoutError::Timeout) => continue,
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
        };

        // The batch window: the reader forwards each buffered chunk as one
        // item (frames the client wrote together), and anything that landed
        // in the channel since joins the same turn's single drain (PRD
        // §16.5 writer priority).
        let mut batch = first;
        while let Ok(queued) = receiver.try_recv() {
            batch.extend(queued);
        }

        let mut end = false;
        for incoming in batch {
            match incoming {
                Incoming::Message(Message::Request(request)) => {
                    if let Some(response) =
                        handle_request(&mut lifecycle, &mut session, &mut fix_hosts, request)
                    {
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
                Incoming::Watch { touched } => handle_watch(&mut session, touched),
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
                Ok(result) => Response::ok(id, result),
                Err(error) => answer_error(id, &error),
            };
            write_message(&mut output, &Message::Response(response))?;
        }
    }

    // #306: the clean-shutdown write, window or no window.
    session.flush_snapshot();

    // The plugin hosts' graceful teardown: `shutdown` per live runtime
    // (the handshake; `Drop` alone is kill-only). Errors log inside.
    fix_hosts.shutdown();

    if reader.is_finished() {
        let _ = reader.join();
    }
    Ok(exit_code.unwrap_or(EXIT_CLEAN))
}

/// Reads frames until the stream or the channel ends. Takes the sender by
/// value: it is the thread's single owner, even though `send` borrows.
///
/// Everything decoded from one buffered chunk is forwarded as one channel
/// item: frames the client wrote together reach the main loop as one turn's
/// batch, decided by the transport's read granularity instead of a race
/// between this thread's per-frame sends and the main loop's drain.
#[allow(clippy::needless_pass_by_value)]
fn reader_loop<R: Read>(mut input: BufReader<R>, sender: mpsc::Sender<Vec<Incoming>>) {
    loop {
        let mut batch = Vec::new();
        let end = loop {
            match codec::read_frame(&mut input) {
                Ok(Some(body)) => {
                    batch.push(decode_body(&body));
                    // An empty buffer means the next frame needs a fresh
                    // read (it would block) — forward what this chunk held.
                    if input.buffer().is_empty() {
                        break None;
                    }
                }
                Ok(None) => break Some(Incoming::Eof),
                Err(FrameError::Io(error)) => {
                    eprintln!("dartq-daemon: input stream failed: {error}");
                    break Some(Incoming::Eof);
                }
                Err(FrameError::Protocol(message)) => {
                    break Some(Incoming::Desynced(message));
                }
            }
        };
        if !batch.is_empty() && sender.send(batch).is_err() {
            return;
        }
        if let Some(end) = end {
            let _ = sender.send(vec![end]);
            return;
        }
    }
}

/// Decodes one well-framed body into what the main loop consumes.
fn decode_body(body: &[u8]) -> Incoming {
    match serde_json::from_slice::<Value>(body) {
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

/// The plugin-dispatch faces' host set (PRD §14): lazily launched at the
/// first `dartq/fix`/`dartq/assists` request — a session that never fixes
/// never pays the plugins' spawn handshakes — and shut down when the
/// session ends. Discovery runs under the session's workspace root (the
/// `<root>/plugins/` convention).
struct FixHosts {
    root: PathBuf,
    hosts: Option<PluginHosts>,
}

impl FixHosts {
    fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            hosts: None,
        }
    }

    /// The live host set, launching it on first use (the manifest budgets'
    /// spawn + handshake cost lands on the first plugin-needing request).
    fn get_or_launch(&mut self) -> &mut PluginHosts {
        self.hosts
            .get_or_insert_with(|| PluginHosts::launch(&self.root))
    }

    /// The graceful teardown; errors log inside (`shutdown` per live
    /// runtime — `Drop` alone is kill-only).
    fn shutdown(self) {
        if let Some(hosts) = self.hosts {
            hosts.shutdown();
        }
    }
}

/// Answers a request, or `None` when the request queued onto the session's
/// scheduler and is answered by the turn's drain instead.
fn handle_request(
    lifecycle: &mut Lifecycle,
    session: &mut Session,
    fix_hosts: &mut FixHosts,
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
        // The overlay faces answer at intake: they mutate session
        // bookkeeping or the worktree (staging, evicting, or merging an
        // agent view), never analyze — no scheduler task, no cancel
        // surface. `overlay.commit`'s merge analysis drains with the write
        // task it stages; the RPC answer is the disk write-back.
        (Lifecycle::Ready, "dartq/overlay.set") => {
            let params = request.params.unwrap_or(Value::Null);
            match commands::parse_overlay_set(&params, session.workspace_root()) {
                Ok((agent, files)) => match session.set_overlay(&agent, files) {
                    Ok(()) => Some(Response::ok(id, overlay_ack())),
                    Err(error) => Some(Response::error(
                        Some(id),
                        codes::INVALID_PARAMS,
                        error.to_string(),
                    )),
                },
                Err(message) => Some(Response::error(Some(id), codes::INVALID_PARAMS, message)),
            }
        }
        (Lifecycle::Ready, "dartq/overlay.commit") => {
            let params = request.params.unwrap_or(Value::Null);
            match commands::parse_overlay_commit(&params) {
                Ok(agent) => match session.commit_overlay(&agent) {
                    Ok(committed) => Some(Response::ok(
                        id,
                        json!({
                            "schema_version": commands::SCHEMA_VERSION,
                            "committed": committed,
                        }),
                    )),
                    Err(error) => Some(answer_error(id, &error)),
                },
                Err(message) => Some(Response::error(Some(id), codes::INVALID_PARAMS, message)),
            }
        }
        (Lifecycle::Ready, "dartq/overlay.clear") => {
            let params = request.params.unwrap_or(Value::Null);
            match commands::parse_overlay_clear(&params) {
                Ok(agent) => {
                    session.clear_overlay(&agent);
                    Some(Response::ok(id, overlay_ack()))
                }
                Err(message) => Some(Response::error(Some(id), codes::INVALID_PARAMS, message)),
            }
        }
        // The plugin-dispatch faces answer at intake too: their work is
        // disk reads plus plugin-process requests (`request/fixes` under
        // the manifests' budgets), never engine analysis — no scheduler
        // task, no cancel surface. The host set launches at the first
        // request that needs it; the session end shuts it down.
        (Lifecycle::Ready, "dartq/fix") => {
            let params = request.params.unwrap_or(Value::Null);
            Some(fix_response(
                fix_hosts,
                &params,
                session.workspace_root(),
                id,
            ))
        }
        (Lifecycle::Ready, "dartq/assists") => {
            let params = request.params.unwrap_or(Value::Null);
            Some(assists_response(
                fix_hosts,
                &params,
                session.workspace_root(),
                id,
            ))
        }
        (Lifecycle::Ready, method) if commands::is_command(method) => {
            // Every `dartq/*` face queues onto the session's read task (the
            // coalesced analysis runs once; the command answers from the
            // post-write state), so `$/cancelRequest` cancels it through
            // the epoch machinery. Bad parameters answer `INVALID_PARAMS`
            // without touching the session.
            let params = request.params.unwrap_or(Value::Null);
            match commands::parse(method, &params, session.workspace_root()) {
                Ok(staged) => {
                    // The agent view and the context tag are validated at
                    // intake (fail loud on a typo instead of silently
                    // answering the default view).
                    if let Some(agent) = staged.agent() {
                        if !session.has_agent(agent) {
                            return Some(Response::error(
                                Some(id),
                                codes::INVALID_PARAMS,
                                format!("unknown agentId: {agent}"),
                            ));
                        }
                    }
                    if let Some(tag) = staged.context() {
                        if !session.has_context(tag) {
                            return Some(Response::error(
                                Some(id),
                                codes::INVALID_PARAMS,
                                format!("unknown context: {tag}"),
                            ));
                        }
                    }
                    session.queue_command(id, staged);
                    None
                }
                Err(message) => Some(Response::error(Some(id), codes::INVALID_PARAMS, message)),
            }
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

/// The overlay faces' acknowledgment body: the contract version, no payload
/// (the staging is the effect; the next request naming the agent observes
/// it).
fn overlay_ack() -> Value {
    json!({ "schema_version": commands::SCHEMA_VERSION })
}

/// One watch batch's intake: the paths split by input kind. `.dart` paths
/// sync their bytes (the didChange-equivalent); a context input (pubspec /
/// package config / analysis options) refreshes the contexts instead —
/// re-discovery re-reads the `.dart` set and re-analysis republishes — while
/// the config bytes themselves never stage as analysis content.
fn handle_watch(session: &mut Session, touched: Vec<PathBuf>) {
    let root = session.workspace_root().to_path_buf();
    let mut sources = Vec::new();
    let mut refreshed = false;
    for path in touched {
        if is_config_input(&path, &root) {
            refreshed = true;
        } else {
            sources.push(path);
        }
    }
    if refreshed && let Err(error) = session.sweep() {
        failed("watch refresh", &error);
    }
    if !sources.is_empty() {
        session.queue_disk_changes(&sources);
    }
}

/// Relative request paths resolve against the session workspace root.
fn resolve_path(root: &Path, path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        root.join(path)
    }
}

/// The `dartq/fix` intake dispatch: the §15.6 documents over the requested
/// paths — built-in lint rows plus the plugin hosts' proposals, every row
/// provenance-tagged, and the hosts' failures in `errors` when any host
/// died. Paths resolve against the session's workspace root.
fn fix_response(
    fix_hosts: &mut FixHosts,
    params: &Value,
    root: &Path,
    id: jsonrpc::RequestId,
) -> Response {
    #[derive(Deserialize)]
    struct FixParams {
        paths: Vec<PathBuf>,
        #[serde(default)]
        verbosity: Verbosity,
    }
    let invalid =
        |message: String| Response::error(Some(id.clone()), codes::INVALID_PARAMS, message);
    if let Err(message) = commands::reject_disk_view_only(params, "dartq/fix") {
        return invalid(message);
    }
    let decoded = match serde_json::from_value::<FixParams>(params.clone()) {
        Ok(decoded) => decoded,
        Err(error) => return invalid(error.to_string()),
    };
    if decoded.paths.is_empty() {
        return invalid("paths must be a non-empty array".into());
    }
    let paths: Vec<PathBuf> = decoded
        .paths
        .into_iter()
        .map(|path| resolve_path(root, path))
        .collect();
    let hosts = fix_hosts.get_or_launch();
    match commands::fix_answer(&paths, decoded.verbosity, hosts) {
        Ok(answer) => Response::ok(id, answer),
        Err(error) => answer_error(id, &error),
    }
}

/// The `dartq/assists` intake dispatch: the plugin hosts' location-directed
/// proposals for one file (`request/fixes` at `[offset, offset + length)`),
/// anchored against the file's on-disk text.
fn assists_response(
    fix_hosts: &mut FixHosts,
    params: &Value,
    root: &Path,
    id: jsonrpc::RequestId,
) -> Response {
    #[derive(Deserialize)]
    struct AssistsParams {
        path: PathBuf,
        #[serde(default)]
        offset: u64,
        #[serde(default)]
        length: u64,
        #[serde(default)]
        verbosity: Verbosity,
    }
    let invalid =
        |message: String| Response::error(Some(id.clone()), codes::INVALID_PARAMS, message);
    if let Err(message) = commands::reject_disk_view_only(params, "dartq/assists") {
        return invalid(message);
    }
    let decoded = match serde_json::from_value::<AssistsParams>(params.clone()) {
        Ok(mut decoded) => {
            decoded.path = resolve_path(root, decoded.path);
            decoded
        }
        Err(error) => return invalid(error.to_string()),
    };
    let hosts = fix_hosts.get_or_launch();
    match fix::assists(&decoded.path, decoded.offset, decoded.length, hosts) {
        Ok(answer) => Response::ok(id, fix::assists_json(&answer, decoded.verbosity)),
        Err(dartq_commands::discover::DiscoverError::Missing(path)) => {
            invalid(format!("Directory or file doesn't exist: {path}"))
        }
        Err(dartq_commands::discover::DiscoverError::Io(io)) => {
            Response::error(Some(id), codes::INTERNAL_ERROR, io.to_string())
        }
    }
}

/// Answers `initialize` with the negotiated capability set: the LSP surface
/// plus the stable `dartq/*` command family, announced as experimental so
/// clients can opt in by name (PRD §15.1: the RPC faces 1:1 with the CLI
/// commands).
fn initialize_response(id: jsonrpc::RequestId) -> Response {
    let dartq_capabilities = commands::METHODS
        .iter()
        .map(|method| {
            (
                (*method).to_string(),
                json!({ "schemaVersion": commands::SCHEMA_VERSION }),
            )
        })
        .collect::<serde_json::Map<String, Value>>();
    Response::ok(
        id,
        json!({
            "capabilities": {
                "positionEncoding": "utf-16",
                "textDocumentSync": 1,
                "experimental": dartq_capabilities,
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
/// answers LSP's `RequestCancelled`; a request whose parameters named no
/// valid target answers `InvalidParams`; anything else is surfaced as
/// `InternalError`.
fn answer_error(id: jsonrpc::RequestId, error: &SessionError) -> Response {
    match error {
        SessionError::Cancelled => Response::error(
            Some(id),
            codes::REQUEST_CANCELLED,
            "request cancelled before execution",
        ),
        SessionError::InvalidParams(message) => {
            Response::error(Some(id), codes::INVALID_PARAMS, message.clone())
        }
        SessionError::Io(io) => Response::error(Some(id), codes::INTERNAL_ERROR, io.to_string()),
        SessionError::Internal(message) => {
            Response::error(Some(id), codes::INTERNAL_ERROR, message.clone())
        }
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
    use crate::test_fixtures::{
        self, BROKEN_MAIN, CLEAN_MAIN, notification, request, spawn, store_root,
    };
    use std::thread;
    use std::time::Duration;

    #[test]
    fn initialize_capabilities_are_pinned() {
        let Response { result, error, id } = initialize_response(RequestId::Num(1));
        let result = result.expect("capabilities result");
        assert_eq!(error, None);
        assert_eq!(id, Some(RequestId::Num(1)));
        assert_eq!(result["capabilities"]["textDocumentSync"], 1);
        assert_eq!(result["capabilities"]["positionEncoding"], "utf-16");
        // The stable `dartq/*` command family, announced as experimental:
        // one entry per method, each carrying the response contract version.
        for method in commands::METHODS {
            assert_eq!(
                result["capabilities"]["experimental"][method]["schemaVersion"],
                commands::SCHEMA_VERSION,
                "{method} is announced"
            );
        }
        assert_eq!(result["serverInfo"]["name"], "dartq");
    }

    /// An idle daemon with a pending save wakes at the debounce deadline and
    /// saves without any further frame (#347): the bounded receive pays off.
    /// The deadline save also leaves the sink clean, so the session-end
    /// flush is a no-op.
    #[test]
    fn idle_server_flushes_at_the_deadline_without_frames() {
        let workspace = test_fixtures::Workspace::new(BROKEN_MAIN);
        let uri = crate::uri::from_path(&workspace.path("lib/main.dart"));
        let mut h = spawn(&workspace, &store_root("idle-flush-unit"));

        h.step(&request(1, "initialize", json!({})), 1);
        h.step(&notification("initialized", json!({})), 1);
        // The edit dirties the snapshot sink at its drain; the probe settles
        // the frame-less step.
        h.step(
            &notification(
                "textDocument/didChange",
                json!({"textDocument": {"uri": uri, "version": 2},
                       "contentChanges": [{"text": CLEAN_MAIN}]}),
            ),
            0,
        );

        crate::probe::reset_counters();
        // No frames at all: past the deadline the bounded receive wakes and
        // the turn-start check saves.
        thread::sleep(crate::session::SNAPSHOT_DEBOUNCE + Duration::from_millis(200));
        assert_eq!(
            crate::probe::counters().1,
            1,
            "the idle daemon saved at the deadline, with no frame to wake it"
        );

        // Session end: the deadline save cleaned the sink — no second save.
        h.step(&request(2, "shutdown", json!(null)), 1);
        let (code, _) = h.exit();
        assert_eq!(code, EXIT_CLEAN);
        assert_eq!(
            crate::probe::counters().1,
            1,
            "the session-end flush is a no-op after the deadline save"
        );
    }
}
