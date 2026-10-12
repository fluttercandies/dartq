//! The server: reader thread → channel → main loop. The reader thread only
//! decodes frames; the main loop (the caller's thread) owns the output, the
//! lifecycle state machine, and the [`Session`], so neither the engine nor
//! the writer ever needs a lock.
//!
//! # Turn shape
//!
//! One turn = block on the next frames, then drain everything already
//! queued into one batch (the rust-analyzer main-loop posture): the reader forwards
//! each buffered chunk as one item, so frames the client wrote together are
//! one turn's intake — capped per turn ([`MAX_TURN_INTAKE`], #947 F2; the
//! channel itself is bounded, so a fast peer backpressures at the
//! transport). N overlay edits and M requests staged by the batch
//! execute at a single [`Session::service`] drain, in the scheduler's
//! writer-priority order — every write applies before any read analyzes. The
//! snapshot debounce is checked at the same turn boundary; while a save is
//! pending the receive is bounded by the window's remainder, so an idle
//! daemon wakes at the deadline to save — still no timer thread.

use std::io::{self, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Instant;

use crossbeam_channel::SelectTimeoutError;
use serde::Deserialize;
use serde_json::{Value, json};

use dartq_commands::Verbosity;

use crate::codec::{self, FrameError};
use crate::commands;
use crate::jsonrpc::{self, Message, Notification, Request, Response, codes};
use crate::lsp::{self, PublishDiagnosticsParams};
use crate::session::{FixJob, Publish, Session, SessionError, Turn};
use crate::watch::{WatchOptions, Watcher, is_config_input};

/// Exit code for a clean session end: `shutdown` + `exit`, or the client
/// simply closed the stream.
pub const EXIT_CLEAN: u8 = 0;
/// Exit code for `exit` without a prior `shutdown` (LSP §lifeCycleMessages).
pub const EXIT_WITHOUT_SHUTDOWN: u8 = 1;
/// Exit code for a session ended by a framing violation (#947 F10): the
/// byte stream lost alignment (malformed header block, duplicate or
/// invalid `Content-Length`, a truncated frame body) and the session ended
/// after one best-effort `ParseError` answer. Supervision reads this code
/// as "the client broke the protocol", never a clean close.
pub const EXIT_PROTOCOL: u8 = 2;

/// The intake channel's bound (#947 F2): at most this many batch items may
/// sit queued while the CPU-bound main loop works. A fast peer outrunning
/// the loop used to stage unbounded decoded messages (each a JSON value
/// holding a whole document — the prior OOM shape); now the reader blocks
/// on a full queue and the transport applies the backpressure.
const MAX_QUEUED_BATCHES: usize = 64;

/// The per-turn intake cap (#947 F2): one loop iteration stages at most
/// this many messages before draining (the extension may overshoot by one
/// reader batch — never drops). The loop starts its next turn immediately,
/// so latency is unaffected; the staged fixes/requests vectors just stay
/// bounded within any single turn.
const MAX_TURN_INTAKE: usize = 64;

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
///
/// # Panics
///
/// Only if the session's fix mailbox is disconnected while the session
/// lives — the session owns the mailbox's sender end, so a disconnect is an
/// internal invariant breach, not a reachable peer-induced state.
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
    let (sender, receiver) = crossbeam_channel::bounded::<Vec<Incoming>>(MAX_QUEUED_BATCHES);
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

    // Ends when the reader thread disconnects or a notification breaks out.
    loop {
        // Turn start: the snapshot debounce window elapses across the
        // receive below — blocking or deadline-bounded — so the flush check
        // happens before intake. (Only tests read the attempt outcome.)
        let _ = session.flush_if_due(Instant::now());

        // A completion or budget expiry that landed while the loop was
        // quiet answers here (the session module's fix-dispatch notes);
        // with nothing to do the call writes nothing.
        write_turn(&mut output, session.complete_fix(None))?;

        // The wait: a select over intake and the fix workers' mailbox, so
        // either wakes the loop — a completion must not sit behind a quiet
        // client. The wait is deadline-bounded while a snapshot save or a
        // fix budget is pending (the loop top fires them); a clean, idle
        // session blocks freely. Either way the loop stays timer-free.
        let deadline = session
            .fix_deadline()
            .into_iter()
            .chain(session.flush_deadline())
            .min();
        let mut select = crossbeam_channel::Select::new();
        let _intake = select.recv(&receiver);
        let fix_done = select.recv(session.fix_mailbox());
        let selected = match deadline {
            None => select.select(),
            Some(deadline) => {
                match select.select_timeout(deadline.saturating_duration_since(Instant::now())) {
                    Ok(selected) => selected,
                    // The loop top re-checks both deadlines: the save or
                    // the expiry fires there. (A disconnect surfaces
                    // through the selected operation's recv below.)
                    Err(SelectTimeoutError) => continue,
                }
            }
        };
        if selected.index() == fix_done {
            // The mailbox never disconnects while the session lives — it
            // owns the sender end, as does the worker holding the hosts.
            let mail = selected
                .recv(session.fix_mailbox())
                .expect("the session owns the fix mailbox sender");
            write_turn(&mut output, session.complete_fix(Some(mail)))?;
            continue;
        }
        let Ok(first) = selected.recv(&receiver) else {
            break;
        };

        // The batch window: the reader forwards each buffered chunk as one
        // item (frames the client wrote together), and anything that landed
        // in the channel since joins the same turn's single drain (PRD
        // §16.5 writer priority) — up to the per-turn cap (#947 F2); the
        // rest stay queued for the next turn.
        let batch = assemble_batch(&receiver, first);

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
                    // The stream broke mid-frame: the exit code must say so
                    // (#947 F10) — a clean 0 would read as an orderly close.
                    exit_code = Some(EXIT_PROTOCOL);
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
        // stale (cancelled) requests answer without a result. A completion
        // that landed mid-batch answers right after (the slot may free and
        // dispatch the next staged fix through the same drain).
        write_turn(&mut output, session.service())?;
        write_turn(&mut output, session.complete_fix(None))?;
    }

    // #306: the clean-shutdown write, window or no window.
    session.flush_snapshot();

    // The plugins' graceful teardown (both the cycle-fed set and the fix
    // faces' hosts): `shutdown` per live runtime (the handshake; `Drop`
    // alone is kill-only). Errors log inside.
    session.shutdown_plugins();

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
/// between this thread's per-frame sends and the main loop's drain. The
/// channel is bounded (#947 F2): a full queue blocks this thread until the
/// main loop catches up — that backpressure is the point, and the reader
/// owns nothing else, so blocking costs nothing; a send past a dead
/// receiver ends the thread.
#[expect(clippy::needless_pass_by_value)] // the reader thread owns the stream for its whole life; a borrow cannot cross the spawn
fn reader_loop<R: Read>(mut input: BufReader<R>, sender: crossbeam_channel::Sender<Vec<Incoming>>) {
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

/// Assembles one turn's batch: `first` plus everything already queued in
/// the channel, up to [`MAX_TURN_INTAKE`] messages (#947 F2). The final
/// extension may overshoot the cap by one reader batch item — nothing is
/// ever dropped; the surplus waits for the next turn, which starts
/// immediately.
fn assemble_batch(
    receiver: &crossbeam_channel::Receiver<Vec<Incoming>>,
    first: Vec<Incoming>,
) -> Vec<Incoming> {
    let mut batch = first;
    while batch.len() < MAX_TURN_INTAKE
        && let Ok(queued) = receiver.try_recv()
    {
        batch.extend(queued);
    }
    batch
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
        // The overlay faces answer at intake (see `overlay_response`).
        (
            Lifecycle::Ready,
            "dartq/overlay.set" | "dartq/overlay.commit" | "dartq/overlay.clear",
        ) => {
            let params = request.params.unwrap_or(Value::Null);
            Some(overlay_response(
                session,
                id,
                request.method.as_str(),
                &params,
            ))
        }
        // The plugin-dispatch faces stage onto the scheduler like reads —
        // the epoch machinery is their cancel surface — and run off the
        // main loop at the drain: the plugin handshakes and the per-hit
        // `request/fixes` round trips never hold the loop (PRD §14.3; the
        // session module's fix-dispatch notes carry the full posture). Bad
        // parameters still answer at intake; the answer itself arrives
        // with the completion, preceded by one `dartq/fixProgress` notice.
        (Lifecycle::Ready, "dartq/fix") => {
            let params = request.params.unwrap_or(Value::Null);
            fix_stage(session, &params, id)
        }
        (Lifecycle::Ready, "dartq/assists") => {
            let params = request.params.unwrap_or(Value::Null);
            assists_stage(session, &params, id)
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
                    if let Some(agent) = staged.agent()
                        && !session.has_agent(agent)
                    {
                        return Some(Response::error(
                            Some(id),
                            codes::INVALID_PARAMS,
                            format!("unknown agentId: {agent}"),
                        ));
                    }
                    if let Some(tag) = staged.context()
                        && !session.has_context(tag)
                    {
                        return Some(Response::error(
                            Some(id),
                            codes::INVALID_PARAMS,
                            format!("unknown context: {tag}"),
                        ));
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

/// Answers the overlay faces at intake: they mutate session bookkeeping or
/// the worktree (staging, evicting, or merging an agent view), never
/// analyze — no scheduler task, no cancel surface. `overlay.commit`'s merge
/// analysis drains with the write task it stages; the RPC answer is the
/// disk write-back.
fn overlay_response(
    session: &mut Session,
    id: jsonrpc::RequestId,
    method: &str,
    params: &Value,
) -> Response {
    match method {
        "dartq/overlay.set" => {
            match commands::parse_overlay_set(params, session.workspace_root()) {
                Ok((agent, files)) => match session.set_overlay(&agent, files) {
                    Ok(()) => Response::ok(id, overlay_ack()),
                    Err(error) => {
                        Response::error(Some(id), codes::INVALID_PARAMS, error.to_string())
                    }
                },
                Err(message) => Response::error(Some(id), codes::INVALID_PARAMS, message),
            }
        }
        "dartq/overlay.commit" => match commands::parse_overlay_commit(params) {
            Ok(agent) => match session.commit_overlay(&agent) {
                Ok(committed) => Response::ok(
                    id,
                    json!({
                        "schema_version": commands::SCHEMA_VERSION,
                        "committed": committed,
                    }),
                ),
                Err(error) => answer_error(id, &error),
            },
            Err(message) => Response::error(Some(id), codes::INVALID_PARAMS, message),
        },
        _ => match commands::parse_overlay_clear(params) {
            Ok(agent) => {
                session.clear_overlay(&agent);
                Response::ok(id, overlay_ack())
            }
            Err(message) => Response::error(Some(id), codes::INVALID_PARAMS, message),
        },
    }
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

/// The `dartq/fix` intake: parameters decode and paths resolve against the
/// session workspace root here (bad parameters answer `INVALID_PARAMS`
/// immediately); the job stages onto the session's scheduler and the
/// completion answers.
fn fix_stage(session: &mut Session, params: &Value, id: jsonrpc::RequestId) -> Option<Response> {
    #[derive(Deserialize)]
    struct FixParams {
        paths: Vec<PathBuf>,
        #[serde(default)]
        verbosity: Verbosity,
    }
    let invalid =
        |message: String| Response::error(Some(id.clone()), codes::INVALID_PARAMS, message);
    if let Err(message) = commands::reject_disk_view_only(params, "dartq/fix") {
        return Some(invalid(message));
    }
    let decoded = match serde_json::from_value::<FixParams>(params.clone()) {
        Ok(decoded) => decoded,
        Err(error) => return Some(invalid(error.to_string())),
    };
    if decoded.paths.is_empty() {
        return Some(invalid("paths must be a non-empty array".into()));
    }
    let root = session.workspace_root().to_path_buf();
    let paths: Vec<PathBuf> = decoded
        .paths
        .into_iter()
        .map(|path| resolve_path(&root, path))
        .collect();
    session.queue_fix(
        id,
        FixJob::Fix {
            paths,
            verbosity: decoded.verbosity,
        },
    );
    None
}

/// The `dartq/assists` intake: the same staging posture as `dartq/fix` —
/// parameters decode and the path resolves at intake; the plugin hosts'
/// location-directed proposals answer with the completion.
fn assists_stage(
    session: &mut Session,
    params: &Value,
    id: jsonrpc::RequestId,
) -> Option<Response> {
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
        return Some(invalid(message));
    }
    let root = session.workspace_root().to_path_buf();
    let decoded = match serde_json::from_value::<AssistsParams>(params.clone()) {
        Ok(mut decoded) => {
            decoded.path = resolve_path(&root, decoded.path);
            decoded
        }
        Err(error) => return Some(invalid(error.to_string())),
    };
    session.queue_fix(
        id,
        FixJob::Assists {
            path: decoded.path,
            offset: decoded.offset,
            length: decoded.length,
            verbosity: decoded.verbosity,
        },
    );
    None
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
            // Full sync (the advertised `textDocumentSync: 1`): every
            // content event must carry the whole document, and the last
            // full-text one wins. A `range` violates that contract; ranged
            // edits are de-scoped (ticket #639 — revisit if the Cockpit e2e
            // needs range sync), so ranged events are counted, logged once,
            // and never applied.
            Ok(change) => {
                let mut full_text = None;
                let mut ranged = 0usize;
                for event in change.content_changes {
                    if event.range.is_none() {
                        full_text = Some(event.text);
                    } else {
                        ranged += 1;
                    }
                }
                match full_text {
                    Some(text) => {
                        if ranged > 0 {
                            eprintln!(
                                "dartq-daemon: ignoring {ranged} ranged edit event(s) alongside \
                                 the full-text event for {} (full-text sync only)",
                                change.text_document.uri
                            );
                        }
                        session.queue_edit(
                            &change.text_document.uri,
                            &text,
                            change.text_document.version,
                        );
                    }
                    None => eprintln!(
                        "dartq-daemon: dropping didChange carrying only {ranged} ranged edit \
                         event(s) for {} (full-text sync only)",
                        change.text_document.uri
                    ),
                }
            }
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

/// Writes one turn's output in contract order: publishes, then the turn's
/// notifications (the fix progress notices ride the same pipeline), then
/// the request answers.
fn write_turn(output: &mut impl Write, turn: Turn) -> io::Result<()> {
    for publish in turn.publishes {
        send_publish(output, publish)?;
    }
    for notification in turn.notifications {
        write_message(output, &Message::Notification(notification))?;
    }
    for (id, answer) in turn.answers {
        let response = match answer {
            Ok(result) => Response::ok(id, result),
            Err(error) => answer_error(id, &error),
        };
        write_message(output, &Message::Response(response))?;
    }
    Ok(())
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
        self, BROKEN_MAIN, CLEAN_MAIN, answered, notification, publishes, request, spawn,
        store_root,
    };
    use std::thread;
    use std::time::Duration;

    #[test]
    fn initialize_capabilities_are_pinned() {
        let Response { result, error, id } = initialize_response(RequestId::Num(1));
        let result = result.expect("capabilities result");
        assert_eq!(error, None);
        assert_eq!(id, Some(RequestId::Num(1)));
        // Full sync (LSP TextDocumentSyncKind::Full = 1): change events
        // carry whole documents only. The advertisement must never promise
        // incremental (2) — ranged edits are de-scoped (ticket #639).
        assert_eq!(
            result["capabilities"]["textDocumentSync"], 1,
            "textDocumentSync must advertise Full (1); ranged edits are not implemented"
        );
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

    /// One turn stages at most [`MAX_TURN_INTAKE`] messages (#947 F2) and
    /// drops none: the surplus stays queued for the next turn.
    #[test]
    fn turn_intake_is_capped_and_never_drops() {
        let (sender, receiver) = crossbeam_channel::unbounded();
        let total = MAX_TURN_INTAKE + 10;
        for _ in 0..total {
            sender.send(vec![Incoming::Eof]).expect("send");
        }
        let batch = assemble_batch(
            &receiver,
            vec![Incoming::Watch {
                touched: Vec::new(),
            }],
        );
        assert_eq!(
            batch.len(),
            MAX_TURN_INTAKE,
            "the turn staged exactly the cap (one Watch + {MAX_TURN_INTAKE} - 1 Eof)"
        );
        // Nothing dropped: the surplus is still in the channel.
        let mut remaining = 0usize;
        while receiver.try_recv().is_ok() {
            remaining += 1;
        }
        assert_eq!(
            batch.len() + remaining,
            total + 1,
            "every message accounted for"
        );
    }

    /// A ranged content event violates the advertised full-sync contract
    /// (`textDocumentSync: 1`, ticket #639 de-scope): it is logged and
    /// dropped without touching the document — the seeded analysis is
    /// unchanged, nothing republishes, and the session stays healthy.
    #[test]
    fn ranged_did_change_is_ignored_and_leaves_state_untouched() {
        let workspace = test_fixtures::Workspace::new(BROKEN_MAIN);
        let uri = crate::uri::from_path(&workspace.path("lib/main.dart"));
        let mut h = spawn(&workspace, &store_root("ranged-change-ignored"));

        h.step(&request(1, "initialize", json!({})), 1);
        h.step(&notification("initialized", json!({})), 1);
        // A ranged event whose text would clear the file if it were
        // applied: the drop must leave the seeded content — and its
        // diagnostics — intact.
        h.step(
            &notification(
                "textDocument/didChange",
                json!({"textDocument": {"uri": uri, "version": 2},
                       "contentChanges": [{"range": {"start": {"line": 0, "character": 0},
                                                      "end": {"line": 0, "character": 6}},
                                           "text": ""}]}),
            ),
            0,
        );
        h.step(&request(2, "dartq/analyze", json!({})), 1);
        h.step(&request(3, "shutdown", json!(null)), 1);
        let (code, transcript) = h.exit();

        assert_eq!(code, EXIT_CLEAN, "the violation never disturbs the session");
        assert_eq!(
            publishes(&transcript).len(),
            1,
            "only the sweep push: the dropped event changed no state"
        );
        let analyze = answered(&transcript)
            .into_iter()
            .find(|frame| frame["id"] == 2)
            .expect("analyze answer");
        assert!(
            analyze["result"]["diagnostics"]
                .as_array()
                .expect("array")
                .iter()
                .any(|d| d["code"] == "uri_does_not_exist"),
            "the document kept its seeded content: {analyze}"
        );
    }

    /// The mixed-batch face of the same de-scope: ranged events riding
    /// alongside full-text events are ignored while the last full-text
    /// event still applies (the conforming path is untouched).
    #[test]
    fn mixed_ranged_and_full_change_applies_only_the_full_text() {
        let workspace = test_fixtures::Workspace::new(BROKEN_MAIN);
        let uri = crate::uri::from_path(&workspace.path("lib/main.dart"));
        let mut h = spawn(&workspace, &store_root("mixed-ranged-change"));

        h.step(&request(1, "initialize", json!({})), 1);
        h.step(&notification("initialized", json!({})), 1);
        h.step(
            &notification(
                "textDocument/didChange",
                json!({"textDocument": {"uri": uri, "version": 2},
                "contentChanges": [
                    {"range": {"start": {"line": 0, "character": 0},
                               "end": {"line": 0, "character": 7}},
                     "text": "garbage"},
                    {"text": CLEAN_MAIN}
                ]}),
            ),
            1,
        );
        h.step(&request(2, "dartq/analyze", json!({})), 1);
        h.step(&request(3, "shutdown", json!(null)), 1);
        let (code, transcript) = h.exit();

        assert_eq!(code, EXIT_CLEAN);
        let pushes = publishes(&transcript);
        assert_eq!(pushes.len(), 2, "sweep push + the full-text clear push");
        assert_eq!(pushes[1]["params"]["version"], 2);
        assert_eq!(pushes[1]["params"]["diagnostics"], json!([]));
        let analyze = answered(&transcript)
            .into_iter()
            .find(|frame| frame["id"] == 2)
            .expect("analyze answer");
        assert_eq!(
            analyze["result"]["diagnostics"],
            json!([]),
            "the full-text event applied; the ranged one changed nothing"
        );
    }

    /// An idle daemon with a pending save wakes at the debounce deadline and
    /// saves without any further frame (#347): the bounded receive pays off.
    /// The deadline save also leaves the sink clean, so the session-end
    /// flush is a no-op.
    ///
    /// Serial phase like the other real-time gates: the window is wall-clock
    /// and the flush tally is process-global, so a parallel test flushing
    /// its own session inside the window would pollute the count.
    #[test]
    #[ignore = "serial phase: real-time debounce window over a process-global probe"]
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
