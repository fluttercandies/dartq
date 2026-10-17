//! The server: reader thread → channel → main loop. The reader thread only
//! decodes frames; the main loop (the caller's thread) owns the output and
//! the [`Session`], so neither the engine nor the writer ever needs a lock.
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
use std::sync::Once;
use std::thread;
use std::time::Instant;

use crossbeam_channel::SelectTimeoutError;
use serde::Deserialize;
use serde_json::{Value, json};

use dartq_commands::Verbosity;

use crate::codec::{self, FrameError};
use crate::commands;
use crate::jsonrpc::{self, Message, Notification, Request, Response, codes};
use crate::session::{FixJob, Publish, Session, SessionError, Turn};
use crate::watch::{WatchOptions, Watcher, is_config_input};
use crate::wire::DiagnosticsParams;

/// Exit code for a clean session end: the client closed the stream.
pub const EXIT_CLEAN: u8 = 0;
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
    /// `--watch` intake: the debounced batch of paths
    /// whose on-disk content changed or vanished since the last batch —
    /// synced like a `dartq/sync` report at the turn's drain.
    Watch {
        touched: Vec<PathBuf>,
    },
}

/// The daemon's single panic-report path (#962 P3-4): installed once per
/// process, this hook replaces the std default's multi-line report — the
/// thread banner, the payload line, the backtrace note — with the shell's
/// one-line `dartq-daemon:` convention. One panic is then one stderr line
/// here plus, at a containment boundary, one recovery line; the payload is
/// never printed twice in two formats. `RUST_BACKTRACE` still opts in to
/// the backtrace, appended after the line.
fn install_panic_hook() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        std::panic::set_hook(Box::new(|info| {
            let thread = std::thread::current();
            eprintln!(
                "dartq-daemon: panic in thread {} at {}: {}",
                thread.name().unwrap_or("<unnamed>"),
                info.location()
                    .map_or("<unknown>".to_owned(), ToString::to_string),
                info.payload_as_str().unwrap_or("opaque panic payload")
            );
            let backtrace = std::backtrace::Backtrace::capture();
            if backtrace.status() == std::backtrace::BacktraceStatus::Captured {
                eprintln!("{backtrace}");
            }
        }));
    });
}

/// Runs one RPC session to its end and returns the process exit code.
///
/// Spawns the reader thread and runs the main loop on the current thread.
/// There is no handshake gate: the session is ready from the start, and the
/// workspace sweep runs before the first turn, so the resident analysis is
/// already warm (and its `dartq/diagnostics` pushes already emitted) before
/// the first client frame lands. With `watch` set it also spawns the one
/// watch thread, whose debounced batches ride the same channel; dropping
/// the watcher at session end stops and joins it. Returns when the input
/// stream ends — a client close is the session's only end signal —
/// (possibly leaving the reader blocked on the next read — under a real
/// process the caller exits immediately, which reaps it; the reader is
/// joined only when it is known to have finished). Input buffering is the
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
    install_panic_hook();
    let (sender, receiver) = crossbeam_channel::bounded::<Vec<Incoming>>(MAX_QUEUED_BATCHES);
    // Held for the session's lifetime: dropping it at return stops and
    // joins the watch thread. A send past a dead receiver ends the thread
    // on its own, too.
    let _watcher = watch.map(|options| Watcher::spawn(options, sender.clone()));
    let reader = thread::Builder::new()
        .name("dartq-daemon-reader".into())
        .spawn(move || reader_loop(BufReader::new(input), sender))?;

    let mut output = output;
    // Set only by a framing desync; every other break is `EXIT_CLEAN`.
    let mut exit_code: Option<u8> = None;

    // The resident posture: discover the workspace and run the first
    // analysis before serving — the sweep's diagnostics pushes (and the
    // session-start advisory) leave with this drain, ahead of anything the
    // client sends.
    if let Err(error) = session.sweep() {
        failed("sweep", &error);
    }
    write_turn(&mut output, session.service())?;

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

        // The contained-panic publish fallback (#1310): a deferred retry
        // that came due re-enqueues the publish-carrying read and drains
        // with no client input; a no-op until then. The receive below is
        // bounded by the retry's deadline, so a quiet session still wakes
        // for it.
        if let Some(turn) = session.retry_deferred_publish(Instant::now()) {
            write_turn(&mut output, turn)?;
        }

        // The wait: a select over intake and the fix workers' mailbox, so
        // either wakes the loop — a completion must not sit behind a quiet
        // client. The wait is deadline-bounded while a snapshot save, a fix
        // budget, or the #1310 publish retry is pending (the loop top fires
        // them); a clean, idle session blocks freely. Either way the loop
        // stays timer-free.
        let deadline = session
            .fix_deadline()
            .into_iter()
            .chain(session.flush_deadline())
            .chain(session.publish_retry_deadline())
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
        // in the channel since joins the same turn's single drain (writer
        // priority) — up to the per-turn cap (#947 F2); the
        // rest stay queued for the next turn.
        let batch = assemble_batch(&receiver, first);

        let mut end = false;
        for incoming in batch {
            match incoming {
                Incoming::Message(Message::Request(request)) => {
                    if let Some(response) = handle_request(&mut session, request) {
                        write_message(&mut output, &Message::Response(response))?;
                    }
                }
                Incoming::Message(Message::Notification(notification)) => {
                    handle_notification(&mut session, &notification);
                }
                Incoming::Message(Message::Response(_)) => {} // clients never send these
                Incoming::Watch { touched } => {
                    handle_watch(&mut session, touched);
                }
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

// The request names `handle_request` intercepts at intake, ahead of the
// `commands::is_command` arm: the overlay faces (answered by
// `overlay_response`) plus the two plugin-dispatch faces. Each name is a
// `const` the match arms below use AS THEIR PATTERN, so the dispatch can
// only serve a name this module declares — and `INTAKE_REQUESTS` is built
// from the same constants, so the served set and the published table are
// one fact, not two copies that could drift. The schema drift gate
// (tests/rpc_schema.rs) asserts every intercepted name is in
// `commands::METHODS` (the exhaustive request family): a new intake arm
// serving an unlisted name — the asymmetry the gate existed to catch —
// now requires extending `METHODS` too.
const OVERLAY_SET: &str = "dartq/overlay.set";
const OVERLAY_COMMIT: &str = "dartq/overlay.commit";
const OVERLAY_CLEAR: &str = "dartq/overlay.clear";
const FIX: &str = "dartq/fix";
const ASSISTS: &str = "dartq/assists";

/// Every request name `handle_request` serves through an intake-literal
/// arm — the faces intercepted before `commands::is_command` routes the
/// rest. `pub` for the drift gate: it must equal the `METHODS`-listed
/// subset the server handles at intake.
pub const INTAKE_REQUESTS: [&str; 5] = [OVERLAY_SET, OVERLAY_COMMIT, OVERLAY_CLEAR, FIX, ASSISTS];

// The notification names `handle_notification` serves, declared the same
// way: the match arms use these constants as their patterns. Notifications
// are deliberately NOT in `commands::METHODS` — that table is the request
// family (a request carries an `id` to answer; a notification does not), so
// the gate pins `SERVED_NOTIFICATIONS` as the closed notification family
// and asserts it stays disjoint from the request methods.
const SYNC: &str = "dartq/sync";
const CANCEL: &str = "dartq/cancel";

/// Every notification name `handle_notification` serves. `pub` for the
/// drift gate; disjoint from `METHODS` by design (see above).
pub const SERVED_NOTIFICATIONS: [&str; 2] = [SYNC, CANCEL];

/// Answers a request, or `None` when the request queued onto the session's
/// scheduler and is answered by the turn's drain instead.
fn handle_request(session: &mut Session, request: Request) -> Option<Response> {
    let id = request.id;
    match request.method.as_str() {
        // The overlay faces answer at intake (see `overlay_response`).
        OVERLAY_SET | OVERLAY_COMMIT | OVERLAY_CLEAR => {
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
        // `request/fixes` round trips never hold the loop (the
        // session module's fix-dispatch notes carry the full posture). Bad
        // parameters still answer at intake; the answer itself arrives
        // with the completion, preceded by one `dartq/fixProgress` notice.
        FIX => {
            let params = request.params.unwrap_or(Value::Null);
            fix_stage(session, &params, id)
        }
        ASSISTS => {
            let params = request.params.unwrap_or(Value::Null);
            assists_stage(session, &params, id)
        }
        method if commands::is_command(method) => {
            // Every `dartq/*` face queues onto the session's read task (the
            // coalesced analysis runs once; the command answers from the
            // post-write state), so `dartq/cancel` cancels it through
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
        method => Some(Response::error(
            Some(id),
            codes::METHOD_NOT_FOUND,
            format!("method not found: {method}"),
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
/// sync their bytes (the same `dartq/sync` path the explicit face takes);
/// a context input (pubspec /
/// package config / analysis options) refreshes the contexts instead —
/// re-discovery re-reads the `.dart` set, re-analysis republishes, and the
/// batch re-evaluates the session's workspace advisory
/// ([`Session::refresh_workspace_advisory`]) — while the config bytes
/// themselves never stage as analysis content.
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
    if refreshed {
        // Every config-input batch re-evaluates the advisory: the watched
        // inputs are exactly the files the verdict reads, so a mid-session
        // `dart pub get` heals it. Source edits never route here, so the
        // source-edit stability stands.
        session.refresh_workspace_advisory();
        if let Err(error) = session.sweep() {
            failed("watch refresh", &error);
        }
    }
    if !sources.is_empty() {
        if let Err(error) = session.queue_disk_changes(&sources) {
            failed("watch sync", &error);
        }
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

/// `dartq/sync` params: the explicit change-report face — the caller names
/// the paths it wrote (the primary change-detection mode; `--watch` is the
/// convenience equivalent) and the session syncs each path's on-disk bytes
/// at the drain. Paths resolve against the session workspace root.
#[derive(Deserialize)]
struct SyncParams {
    paths: Vec<PathBuf>,
}

/// Handles one notification.
fn handle_notification(session: &mut Session, notification: &Notification) {
    let params = notification.params.clone().unwrap_or(Value::Null);
    match notification.method.as_str() {
        SYNC => match serde_json::from_value::<SyncParams>(params) {
            Ok(sync) => {
                let root = session.workspace_root().to_path_buf();
                let paths: Vec<PathBuf> = sync
                    .paths
                    .into_iter()
                    .map(|path| resolve_path(&root, path))
                    .collect();
                // A notification has no id to answer: the structured
                // `InvalidParams` error (the live-path ceiling's trip,
                // #1887) reports through the unanswerable-failure idiom.
                if let Err(error) = session.queue_disk_changes(&paths) {
                    failed(SYNC, &error);
                }
            }
            Err(error) => eprintln!("dartq-daemon: dropping malformed dartq/sync: {error}"),
        },
        CANCEL => match params.get("id").and_then(jsonrpc::RequestId::from_value) {
            Some(id) => session.cancel_request(&id),
            None => eprintln!("dartq-daemon: dropping dartq/cancel without an id"),
        },
        // Unknown notifications are ignored.
        _ => {}
    }
}

/// Maps a turn answer's failure to its response: a stale (cancelled) read
/// answers `RequestCancelled`; a request whose parameters named no
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
    let params = DiagnosticsParams {
        uri: publish.uri,
        diagnostics: publish.diagnostics,
    };
    write_message(
        output,
        &Message::Notification(Notification::new(
            "dartq/diagnostics",
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
    use crate::test_fixtures::{
        self, BROKEN_MAIN, CLEAN_MAIN, answered, framed, notification, publishes, request, spawn,
        store_root,
    };
    use dartq_snapshot::{DEFAULT_CAPACITY_BYTES, SnapshotConfig, SnapshotStore};
    use std::fs;
    use std::io::BufRead;
    use std::sync::mpsc;
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::Duration;

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

    /// The session is ready from `run()`'s start — no handshake gate: the
    /// startup sweep's publish lands ahead of the first client frame, an
    /// early request answers, and EOF ends the session cleanly.
    #[test]
    fn session_is_ready_without_a_handshake() {
        let workspace = test_fixtures::Workspace::new(BROKEN_MAIN);
        let uri = crate::uri::from_path(&workspace.path("lib/main.dart"));
        let mut h = spawn(&workspace, &store_root("no-handshake"));

        // The startup drain already emitted the sweep's publish; the first
        // request answers normally (no lifecycle gate).
        h.step(&request(1, "dartq/analyze", json!({})), 1);
        let (code, transcript) = h.join_ended();

        assert_eq!(code, EXIT_CLEAN);
        let pushes = publishes(&transcript);
        assert_eq!(pushes.len(), 1, "the sweep push alone: {pushes:?}");
        assert_eq!(pushes[0]["params"]["uri"], uri);
        assert!(
            pushes[0]["params"]["diagnostics"]
                .as_array()
                .expect("array")
                .iter()
                .any(|d| d["code"] == "uri_does_not_exist"),
            "the sweep reports the seeded error: {pushes:?}"
        );
        let analyze = answered(&transcript)
            .into_iter()
            .find(|frame| frame["id"] == 1)
            .expect("analyze answer");
        assert!(
            analyze["result"]["diagnostics"]
                .as_array()
                .expect("array")
                .iter()
                .any(|d| d["code"] == "uri_does_not_exist"),
            "the pull face agrees: {analyze}"
        );
    }

    /// `dartq/sync` is the explicit change-report face: a path the caller
    /// wrote syncs from disk at the drain and its fresh diagnostic set
    /// pushes.
    #[test]
    fn dartq_sync_republishes_the_reported_path() {
        let workspace = test_fixtures::Workspace::new(BROKEN_MAIN);
        let main = workspace.path("lib/main.dart");
        let uri = crate::uri::from_path(&main);
        let mut h = spawn(&workspace, &store_root("sync-push"));

        fs::write(&main, CLEAN_MAIN).expect("rewrite");
        h.step(
            &notification("dartq/sync", json!({"paths": ["lib/main.dart"]})),
            1,
        );
        let (code, transcript) = h.join_ended();

        assert_eq!(code, EXIT_CLEAN);
        let pushes = publishes(&transcript);
        assert_eq!(pushes.len(), 2, "sweep push + the sync's clear push");
        assert_eq!(pushes[1]["params"]["uri"], uri);
        assert_eq!(pushes[1]["params"]["diagnostics"], json!([]));
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
        let main = workspace.path("lib/main.dart");
        let mut h = spawn(&workspace, &store_root("idle-flush-unit"));

        // The sync dirties the snapshot sink at its drain.
        fs::write(&main, CLEAN_MAIN).expect("rewrite");
        h.step(
            &notification("dartq/sync", json!({"paths": ["lib/main.dart"]})),
            1,
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
        let (code, _) = h.join_ended();
        assert_eq!(code, EXIT_CLEAN);
        assert_eq!(
            crate::probe::counters().1,
            1,
            "the session-end flush is a no-op after the deadline save"
        );
    }

    // -- Server-level loop pins -------------------------------------------
    //
    // The shared fixture's harness cannot serve these: it spawns the loop
    // without watch (the watch pin needs a watcher) and its thread-local
    // panic arm cannot reach a spawned loop thread (the quiet-retry pin must
    // arm the containment on the session-owner thread). Both harness pieces
    // mirror the rpc_commands watch-test idiom: an mpsc feed read as a
    // stream, an appending output sink, and a bounded transcript poll.

    /// The client end of the transport: byte chunks read as a stream,
    /// disconnection is EOF.
    struct FeedClient {
        receiver: mpsc::Receiver<Vec<u8>>,
        pending: Vec<u8>,
    }

    impl Read for FeedClient {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.pending.is_empty() {
                self.pending = self.receiver.recv().map_err(|_| {
                    io::Error::new(io::ErrorKind::UnexpectedEof, "client disconnected")
                })?;
            }
            let take = buf.len().min(self.pending.len());
            buf[..take].copy_from_slice(&self.pending[..take]);
            self.pending.drain(..take);
            Ok(take)
        }
    }

    impl BufRead for FeedClient {
        fn fill_buf(&mut self) -> io::Result<&[u8]> {
            if self.pending.is_empty() {
                self.pending = self.receiver.recv().map_err(|_| {
                    io::Error::new(io::ErrorKind::UnexpectedEof, "client disconnected")
                })?;
            }
            Ok(&self.pending)
        }

        fn consume(&mut self, amount: usize) {
            self.pending.drain(..amount);
        }
    }

    /// The server's output side: everything written, appended, decoded on
    /// demand.
    #[derive(Clone, Default)]
    struct TestOutput(Arc<Mutex<Vec<u8>>>);

    impl Write for TestOutput {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().expect("output lock").extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// The output buffer decoded into transcript frames.
    fn transcript_of(output: &TestOutput) -> Vec<Value> {
        test_fixtures::frames(&output.0.lock().expect("output lock").clone())
    }

    /// Polls `probe` every 10 ms until it answers, bounded by the settle
    /// window.
    fn wait_for<T>(mut probe: impl FnMut() -> Option<T>, message: &str) -> T {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(found) = probe() {
                return found;
            }
            assert!(Instant::now() < deadline, "{message}");
            thread::sleep(Duration::from_millis(10));
        }
    }

    /// A watch batch converges the session to the touched disk state: the
    /// debounced batch syncs the rewrite, its diff pushes the clear, and
    /// the analyze pull taken after the push answers the clean set. The
    /// pull waits on the push — the same order the rpc watch pin takes —
    /// because watch events are hints, not facts: a pull raced ahead of the
    /// debounced batch answers the pre-batch state by design.
    #[test]
    fn watch_batch_converges_the_view() {
        let workspace = test_fixtures::Workspace::new(BROKEN_MAIN);
        let main = workspace.path("lib/main.dart");
        let store = SnapshotStore::open(SnapshotConfig {
            root: Some(store_root("watch-converges")),
            capacity_bytes: DEFAULT_CAPACITY_BYTES,
        })
        .expect("store opens on a temp root");
        let session = Session::with_store(workspace.engine_config(), store, None).expect("session");
        let (client, receiver) = mpsc::channel::<Vec<u8>>();
        let output = TestOutput::default();
        let server_output = output.clone();
        let root = workspace.root.clone();
        let server = thread::spawn(move || {
            run(
                BufReader::new(FeedClient {
                    receiver,
                    pending: Vec::new(),
                }),
                server_output,
                session,
                Some(WatchOptions {
                    root,
                    poll_interval: Duration::from_millis(10),
                }),
            )
        });

        // Settle the watcher's baseline scan, then make exactly one watched
        // change. The batch flushes on the second 10 ms tick.
        thread::sleep(Duration::from_millis(150));
        fs::write(&main, CLEAN_MAIN).expect("watched rewrite");

        // The batch's diff pushes the clear — the batch applied and the
        // view converged to the touched disk state.
        wait_for(
            || {
                transcript_of(&output)
                    .iter()
                    .any(|frame| {
                        frame["method"] == "dartq/diagnostics"
                            && frame["params"]["uri"]
                                .as_str()
                                .is_some_and(|uri| uri.ends_with("lib/main.dart"))
                            && frame["params"]["diagnostics"] == json!([])
                    })
                    .then_some(())
            },
            "the watch batch never pushed the clear",
        );

        client
            .send(framed(&request(2, "dartq/analyze", json!({}))))
            .expect("analyze send");
        let analyze = wait_for(
            || {
                transcript_of(&output)
                    .iter()
                    .find(|frame| frame.get("id") == Some(&json!(2)))
                    .cloned()
            },
            "the analyze request never answered",
        );
        drop(client);
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline && !server.is_finished() {
            thread::sleep(Duration::from_millis(1));
        }
        assert!(server.is_finished(), "the server did not end");
        let code = server
            .join()
            .expect("server thread does not panic")
            .expect("transport does not fail on memory pipes");
        assert_eq!(code, EXIT_CLEAN);
        assert_eq!(
            analyze["result"]["diagnostics"],
            json!([]),
            "the view converged to the touched disk state: {analyze}"
        );
    }

    /// A quiet daemon — zero client input after the contained turn — wakes
    /// at the deferred publish's deadline through the receive-bound
    /// deadline chain (the loop's deadline-bounded select) and emits the
    /// publish through the loop-top retry call. The session unit tests
    /// drive `retry_deferred_publish` directly; this pin exercises the
    /// wiring end to end, over the wire.
    ///
    /// Serial phase like the idle-flush gate: the assertion is a wall-clock
    /// bound. The retry fires ~100 ms after the containment, while the
    /// snapshot-flush deadline that could substitute for the wake sits at
    /// ~500 ms — the 400 ms bound tells them apart.
    #[test]
    #[ignore = "serial phase: real-time retry window over a wall-clock bound"]
    fn quiet_server_emits_the_deferred_publish_at_its_deadline() {
        let workspace = test_fixtures::Workspace::new(BROKEN_MAIN);
        let main = workspace.path("lib/main.dart");
        let uri = crate::uri::from_path(&main);
        let store = SnapshotStore::open(SnapshotConfig {
            root: Some(store_root("quiet-retry-publish")),
            capacity_bytes: DEFAULT_CAPACITY_BYTES,
        })
        .expect("store opens on a temp root");
        let session = Session::with_store(workspace.engine_config(), store, None).expect("session");
        let (client, receiver) = mpsc::channel::<Vec<u8>>();
        let output = TestOutput::default();
        let server_output = output.clone();

        // The loop runs on THIS thread: the probe's read-panic trigger is
        // thread-local (the containment pin arms it on the session-owner
        // thread), so a spawned-thread harness cannot reach it. A feeder
        // thread plays the client. The startup sweep's read is the first
        // group — skip it so the sync batch's base group takes the hit.
        crate::probe::arm_read_panic_on_group(1);
        // The feeder's view of the same buffer (the loop's writer end is the
        // third handle, moved into `run` below).
        let feeder_output = output.clone();
        let feeder_uri = uri.clone();
        let feeder = thread::spawn(move || {
            let output = feeder_output;
            // The panicking batch, in writer-priority order: the sync stages
            // the write and the publish-carrying read; the analyze request
            // makes the containment wire-visible. The drain applies the
            // write (diff due), the base group panics at the containment
            // boundary, and the retry is scheduled — the client then goes
            // silent.
            fs::write(&main, CLEAN_MAIN).expect("rewrite");
            let mut batch = Vec::new();
            batch.extend_from_slice(&framed(&notification(
                "dartq/sync",
                json!({"paths": ["lib/main.dart"]}),
            )));
            batch.extend_from_slice(&framed(&request(2, "dartq/analyze", json!({}))));
            client.send(batch).expect("batch send");
            let answer = wait_for(
                || {
                    transcript_of(&output)
                        .iter()
                        .find(|frame| frame.get("id") == Some(&json!(2)))
                        .cloned()
                },
                "the panicked turn never answered",
            );
            assert_eq!(
                answer["error"]["code"],
                codes::INTERNAL_ERROR,
                "the containment answers the read internal error: {answer}"
            );

            // Quiet: no further input. The deferred publish must surface
            // through the deadline chain; the wait is bounded inside the
            // snapshot-flush window (~500 ms) that could otherwise mask the
            // wake.
            let quiet_start = Instant::now();
            wait_for(
                || {
                    transcript_of(&output)
                        .iter()
                        .find(|frame| {
                            frame["method"] == "dartq/diagnostics"
                                && frame["params"]["uri"] == feeder_uri
                        })
                        .cloned()
                },
                "the deferred publish never surfaced with the daemon quiet",
            );
            let surfaced = quiet_start.elapsed();
            assert!(
                surfaced < Duration::from_millis(400),
                "the deferred publish surfaced at {surfaced:?}: the retry-deadline                  wake must beat the ~500 ms snapshot flush"
            );
        });

        let code = run(
            BufReader::new(FeedClient {
                receiver,
                pending: Vec::new(),
            }),
            server_output,
            session,
            None,
        )
        .expect("transport does not fail on memory pipes");
        feeder.join().expect("the feeder's pins hold");

        assert_eq!(code, EXIT_CLEAN);
        // The startup sweep pushed the seeded error; the deferred diff —
        // drained by the retry with no client input — is the post-write
        // base view.
        let transcript = transcript_of(&output);
        let pushes = publishes(&transcript);
        let last = pushes.last().expect("at least the sweep push");
        assert_eq!(last["params"]["uri"], uri);
        assert_eq!(
            last["params"]["diagnostics"],
            json!([]),
            "the deferred push is the post-write base view: {pushes:?}"
        );
    }
}
