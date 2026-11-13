//! The one-shot faces' resident routing (#2627/#2629): `dartq analyze`,
//! `dartq check`, and the answer faces (`query` and its batch, `explain`,
//! `impact`, `rename`'s dry run, `resolve`, `complete`) consult the
//! worktree's resident session by default; `--cold` (the faces that carry
//! it) forces the in-process path.
//!
//! # Why
//!
//! The AI-first posture is one resident engine per worktree
//! (`dartq session`): a cold one-shot pays the whole engine assembly per
//! call (measured 3.9–5.0 s / 430 MB on a Flutter package, recon #2620)
//! where the resident session answers the same question from the
//! maintained analysis. The one-shot faces were the last CLI surface
//! that never consulted it.
//!
//! # Shape
//!
//! * `analyze` (the engine face) elects or attaches the session —
//!   spawning and waiting for bring-up when none is live, the election
//!   the `session` verb owns (`dartq_session::ensure` + the detached
//!   serve spawn) — and asks `dartq/analyze` for the same targets.
//! * `check` (and `analyze --fast`) only ever *attaches* a live session,
//!   never spawning: the M0 pipeline has no resident state to reuse, so
//!   a spawn would spend the engine bring-up a syntax sweep must not
//!   pay. The daemon's `dartq/check` runs the very same disk pipeline
//!   (`dartq_commands::check::analyze_targets`) and renders through the
//!   same writers.
//! * Both requests carry `format` (#2627's wire extension): the daemon
//!   renders the stdout bytes through the ONE writer per format the
//!   cold face prints with (`dartq_commands::render`,
//!   `dartq_diagnostics::write_json`), so the routed bytes are the cold
//!   bytes structurally — one renderer per format, by relocation rather
//!   than reconstruction (the oracle JSON envelope's key order does not
//!   survive a value round trip).
//!
//! # Freshness
//!
//! The routed answer reflects the on-disk tree at call time by
//! construction: every daemon drain with engine work re-walks the
//! workspace under (mtime, size) stamps ahead of the analysis (the
//! session's refresh seam), the check face re-discovers and re-parses
//! from disk on every request, and a moved pubspec / package config /
//! analysis options re-derives the analysis universe itself before the
//! answer (#2675 — a mid-session `pub get` swaps the session's default
//! context onto the fresh package graph). The answer carries the
//! universe's package-config stamp besides, and the caller verifies it
//! against the disk (`universe_matches_disk`): a daemon generation that
//! cannot invalidate sees its answer refused here, never printed.
//!
//! # The ladder
//!
//! Daemon unreachable, a failed handshake, or a domain error answer:
//! re-elect once (the election reaps stale generations and respawns),
//! then fall back to the in-process path with one stderr advisory —
//! `dartq: resident session unavailable (<reason>); analyzing
//! in-process`. Exit codes never change: the data-found ladder is
//! computed from the answer's severity counts with the caller's fatal
//! flags, the same arithmetic the cold path runs.
//!
//! Invocations the resident engine cannot serve faithfully —
//! `--overlay`, `--packages`, `--enable-experiment`, `--no-plugins`,
//! `--memory-limit`, `--trace-events`, `--cache-size-limit`,
//! multi-workspace target sets, an unresolvable SDK root — route never
//! and run cold silently: the cold path owns those semantics and their
//! errors.
//!
//! # The answer faces (#2629)
//!
//! The value-answer faces consult the same resident layer, their daemon
//! counterparts carrying a `format` parameter that switches the answer to
//! the rendered stdout bytes of the one writer per format the cold face
//! prints through (`dartq_commands::render`) — the routed bytes are the
//! cold bytes structurally, and the value faces' exit tier is 0 on an
//! answered document (their usage errors re-derive on the cold fallback).
//! Posture:
//!
//! * Every answer face **attaches** a live session, never spawning — the
//!   check face's doctrine. The election (spawn + wait) belongs to the
//!   faces that own a worktree's residency (`session start`, `analyze`);
//!   a one-shot read that spawned would leave a resident engine behind an
//!   invocation that asked for one answer, and its first call pays the
//!   same engine assembly either way. A live session answers from its
//!   maintained view; no live session falls straight through to the cold
//!   path with no advisory (nothing was lost).
//! * The engine-view answers (`query` every kind, `query batch`,
//!   `impact`, `rename`'s dry run, `resolve`, `complete`) carry the
//!   universe stamp the caller verifies exactly as analyze's does
//!   (`universe_matches_disk`).
//! * `rename --write` routes never: the write applies through the CLI
//!   face's own engine-local splice machinery (drift checks against the
//!   analyzed view), which the daemon must not remote — the daemon
//!   answers the edit set, the write stays here.
//! * `explain`'s rendered answer carries no universe stamp: the daemon
//!   rebuilds a per-query engine over the on-disk base (the visibility
//!   table's posture — a warm session engine would silently miss
//!   declarations), so its bytes read the disk fresh every time.

use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use dartq_session::{Ensured, Layout, STALE_HEARTBEAT, SessionState, SessionTriple, ensure};

use crate::exit;
use crate::mcp::{CALL_TIMEOUT, ProxyError};
use crate::session::sdk_root_for;

// ---------------------------------------------------------------------------
// The shared session transport (the `mcp` and `debug` faces proxy their
// `dartq/*` requests through the same machinery — the [`ProxyError`]
// failure contract and the [`CALL_TIMEOUT`] bound live beside
// `mcp::daemon_call`, the proxy API they describe)
// ---------------------------------------------------------------------------

/// One daemon request over the session socket, bounded by `timeout`.
pub(crate) fn session_call(
    socket: &Path,
    method: &str,
    params: &Value,
    timeout: Duration,
) -> Result<Value, ProxyError> {
    let body = session_call_frame(socket, method, params, timeout)?;
    let answer: Value =
        serde_json::from_slice(&body).map_err(|error| ProxyError::Transport(error.to_string()))?;
    answer_result(answer)
}

/// Maps one parsed response frame onto the call's answer: the error
/// answers become the daemon error, a success answer yields its result
/// (`{}` when absent).
fn answer_result(answer: Value) -> Result<Value, ProxyError> {
    if let Some(error) = answer.get("error") {
        return Err(ProxyError::Daemon(
            // The daemon's error answers always carry both fields
            // (`Response::error`); -32603 is the JSON-RPC fallback tier.
            error["code"].as_i64().unwrap_or(-32603),
            error["message"].as_str().unwrap_or_default().to_owned(),
        ));
    }
    Ok(answer.get("result").cloned().unwrap_or_else(|| json!({})))
}

/// Sends one daemon request and reads until the daemon's response frame,
/// returning the response frame's raw body bytes — unparsed (#4190):
/// the routed faces cut the stdout member out of these bytes without
/// ever materializing a value tree; the parsed face (`session_call`,
/// the MCP proxy behind it) parses them whole.
fn session_call_frame(
    socket: &Path,
    method: &str,
    params: &Value,
    timeout: Duration,
) -> Result<Vec<u8>, ProxyError> {
    let mut stream =
        UnixStream::connect(socket).map_err(|error| ProxyError::Transport(error.to_string()))?;
    // The bound covers the whole round trip: a wedged engine (or a
    // wedged serve between clients) answers the timeout error, never a
    // hang.
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|error| ProxyError::Transport(error.to_string()))?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(|error| ProxyError::Transport(error.to_string()))?;
    let body = serde_json::to_string(&json!({
        "jsonrpc": "2.0", "id": 1, "method": method, "params": params,
    }))
    .map_err(|error| ProxyError::Transport(error.to_string()))?;
    write!(stream, "Content-Length: {}\r\n\r\n{body}", body.len())
        .map_err(|error| ProxyError::Transport(error.to_string()))?;
    stream
        .flush()
        .map_err(|error| ProxyError::Transport(error.to_string()))?;
    // The daemon streams publishes and progress notifications ahead of
    // the answer on the same connection (#2596: a resident that picked up
    // an out-of-band edit emits its diagnostics pushes first) — the proxy
    // reads past every notification frame until the response (the frame
    // carrying our id).
    let wedged = || {
        ProxyError::Transport(format!(
            "the engine did not answer {method} within {timeout:?} — \
             the session may be wedged; check `dartq session status`"
        ))
    };
    let mut leftover = Vec::new();
    loop {
        let body = match read_frame_body(&mut stream, &mut leftover) {
            Ok(body) => body,
            Err(ReadError::Wedged) => return Err(wedged()),
            Err(ReadError::Io(error)) => return Err(ProxyError::Transport(error.to_string())),
            Err(ReadError::Malformed(message)) => return Err(ProxyError::Transport(message)),
        };
        if is_notification(&body) {
            continue;
        }
        return Ok(body);
    }
}

/// Whether a raw frame is a notification (id-less — read past it): the
/// daemon's writer encodes every frame compact with `jsonrpc` first and
/// the notifications' second member `method` (`Message::to_value`'s
/// insertion order, pinned by `preserve_order`; `write_wire_response`
/// splices the same response prefix), so the prefix answers without a
/// parse — the raw twin of the parsed loop's `get("id").is_none()`.
fn is_notification(body: &[u8]) -> bool {
    body.starts_with(b"{\"jsonrpc\":\"2.0\",\"method\"")
}

/// One framed-read failure: the socket timed out (the wedge signal,
/// #2587), the transport broke, or the frame was not a valid
/// Content-Length-framed JSON message.
pub(crate) enum ReadError {
    Wedged,
    Io(std::io::Error),
    Malformed(String),
}

impl std::fmt::Debug for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Wedged => f.write_str("Wedged"),
            Self::Io(error) => write!(f, "Io({error})"),
            Self::Malformed(message) => write!(f, "Malformed({message})"),
        }
    }
}

/// Reads one Content-Length framed JSON message (the one transport's
/// reader — the `mcp` face's frame tests exercise it directly). The
/// header reads in stack-buffered chunks with a memchr scan for the
/// `\r\n\r\n` terminator (#4260) — one read syscall per chunk, not the
/// one-byte-per-syscall walk the terminator walk was. `pending` carries
/// whatever a chunk read brought past this frame — the next frame's
/// leading bytes; a caller reading several frames off one connection
/// passes the same vec to every call, one-shot callers `&mut Vec::new()`.
pub(crate) fn read_frame(
    stream: &mut UnixStream,
    pending: &mut Vec<u8>,
) -> Result<Value, ReadError> {
    let body = read_frame_body(stream, pending)?;
    serde_json::from_slice(&body).map_err(|error| ReadError::Malformed(error.to_string()))
}

/// The most body bytes one frame header may announce: the analyzer-host
/// face path's `MAX_FACE_BYTES` cap (1 GiB) mirrored on the resident
/// transport. That constant is `pub(crate)` to `dartq-analyzer-host`, so
/// the number is restated here rather than imported; both paths refuse an
/// over-cap announcement with their structured error, so no wire-driven
/// allocation is ever unbounded.
const MAX_FRAME_BODY_BYTES: usize = 1 << 30;

/// Reads one Content-Length framed message's raw body bytes — the
/// framed reader's unparsed half (#4190): the header scan is the
/// transport's own (#4260); the body returns without a JSON parse for
/// the callers that splice the frame instead of materializing it.
pub(crate) fn read_frame_body(
    stream: &mut UnixStream,
    pending: &mut Vec<u8>,
) -> Result<Vec<u8>, ReadError> {
    read_frame_body_capped(stream, pending, MAX_FRAME_BODY_BYTES)
}

/// [`read_frame_body`] with the announcement cap as a parameter — the
/// test seam for the over/under-cap arms without a gigabyte fixture.
fn read_frame_body_capped(
    stream: &mut UnixStream,
    pending: &mut Vec<u8>,
    cap: usize,
) -> Result<Vec<u8>, ReadError> {
    let map_error = |error: std::io::Error| {
        if matches!(
            error.kind(),
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
        ) {
            ReadError::Wedged
        } else {
            ReadError::Io(error)
        }
    };
    // A frame's header is `Content-Length: N\r\n\r\n` (a couple dozen
    // bytes); 512 covers any multi-header spelling the daemon sends.
    let mut buf = [0u8; 512];
    // The previous call's over-read plays the first chunk's role.
    let mut filled = pending.len().min(buf.len());
    buf[..filled].copy_from_slice(&pending[..filled]);
    let carried_pending = filled;
    // Bytes already confirmed terminator-free: each pass scans only the
    // unscanned tail, rewinding three bytes in case the terminator
    // straddled the pass boundary.
    let mut scanned = 0usize;
    let header_end;
    loop {
        let look_from = scanned.saturating_sub(3);
        if let Some(index) = memchr::memmem::find(&buf[look_from..filled], b"\r\n\r\n") {
            header_end = look_from + index + 4;
            break;
        }
        scanned = filled;
        if filled == buf.len() {
            return Err(ReadError::Malformed("frame header too long".to_owned()));
        }
        // One read syscall per pass.
        let n = match stream.read(&mut buf[filled..]) {
            Ok(0) => {
                return Err(ReadError::Io(std::io::Error::from(
                    std::io::ErrorKind::UnexpectedEof,
                )));
            }
            Ok(n) => n,
            Err(error) => return Err(map_error(error)),
        };
        filled += n;
    }
    let header = String::from_utf8_lossy(&buf[..header_end]);
    let length: usize = header
        .lines()
        .find_map(|line| {
            line.split_once(':').and_then(|(name, value)| {
                (name.trim().eq_ignore_ascii_case("content-length"))
                    .then(|| value.trim().parse::<usize>().ok())
                    .flatten()
            })
        })
        .ok_or_else(|| ReadError::Malformed("missing Content-Length".to_owned()))?;
    // Fail closed before allocating or reading anything: an announcement
    // over the cap is a corrupt or mismatched peer, and the caller's
    // Malformed arm drops the connection (the cold fallback behind it).
    if length > cap {
        return Err(ReadError::Malformed(format!(
            "frame announces {length} bytes, over the {cap}-byte cap"
        )));
    }
    // The chunk may already carry body bytes — this frame's start, and
    // (rarely, when a read spans a frame boundary) the next frame's
    // leading bytes, which return to `pending`.
    let mut consumed = header_end;
    if carried_pending > 0 {
        pending.clear();
    }
    let carried = filled - consumed;
    let take = carried.min(length);
    let mut body = vec![0u8; length];
    body[..take].copy_from_slice(&buf[consumed..consumed + take]);
    consumed += take;
    if consumed < filled {
        pending.clear();
        pending.extend_from_slice(&buf[consumed..filled]);
    }
    stream.read_exact(&mut body[take..]).map_err(map_error)?;
    Ok(body)
}

// ---------------------------------------------------------------------------
// The routed answer (#4190): splice the frame, never parse it
// ---------------------------------------------------------------------------

/// One routed answer cut from the daemon's raw response frame (#4190).
/// The frame's one expensive member — the rendered stdout — is located
/// by scan and unescaped **in place** into the frame's own buffer (every
/// JSON escape encodes at least as many bytes as it decodes to, so the
/// write cursor never passes the read cursor: no allocation beyond the
/// frame itself, no value tree, no second payload copy). Every other
/// member — `counts`, `advisory`, the universe stamp — parses as a small
/// value over the frame with the stdout body spliced empty: a few
/// hundred bytes.
///
/// The scan is structural by construction: a JSON string escapes every
/// quote it contains, so the byte sequence `"stdout"` can only be a
/// member's key — no payload can forge one — and the spliced-empty
/// parse fails closed (the malformed transport error, the cold fallback
/// behind it) if any span were wrong. Frames without a stdout member —
/// error answers, or a result without the rendered face — parse whole:
/// stdout is the protocol's only large member, so the no-stdout arm is
/// small by shape.
#[derive(Debug)]
struct RoutedAnswer {
    /// The response frame, its stdout body unescaped in place.
    frame: Vec<u8>,
    /// The stdout member's unescaped span; `None` when the answer
    /// carried no rendered face (the caller's fallback arm).
    stdout: Option<std::ops::Range<usize>>,
    /// The frame with the stdout body spliced empty, parsed: the
    /// envelope's small members exactly as the parsed path reads them.
    small: Value,
}

impl RoutedAnswer {
    /// The small parsed result object (the parsed path's answer — an
    /// absent result indexes as null, the parsed `{}`'s own posture).
    fn result(&self) -> &Value {
        match self.small.get("result") {
            Some(result) => result,
            None => &Value::Null,
        }
    }

    /// The unescaped rendered stdout. The callers gate on presence (the
    /// `stdout` member's absence is the fallback arm, never a print) —
    /// a missing field here is a program bug, not a runtime case.
    fn stdout(&self) -> &[u8] {
        let span = self
            .stdout
            .clone()
            .expect("the routed answer carries the rendered stdout");
        &self.frame[span]
    }
}

/// One routed daemon round trip: the raw response frame cut into a
/// [`RoutedAnswer`] (the routed faces' `session_call` — the answer's
/// cost is the splice, never a parse).
fn session_call_routed(
    socket: &Path,
    method: &str,
    params: &Value,
    timeout: Duration,
) -> Result<RoutedAnswer, ProxyError> {
    routed_answer(session_call_frame(socket, method, params, timeout)?)
}

/// Cuts one routed answer out of the daemon's raw response frame. A
/// frame with a stdout member yields the spliced answer; one without
/// parses whole and keeps the parsed transport's semantics — the error
/// answers map to the daemon error, a stdout-less result is the
/// caller's fallback arm.
fn routed_answer(mut frame: Vec<u8>) -> Result<RoutedAnswer, ProxyError> {
    let malformed = |why: String| ProxyError::Transport(format!("malformed answer frame: {why}"));
    let (span, small) = match locate_stdout_string(&frame) {
        Some(span) => {
            // The small frame: the envelope with the stdout body spliced
            // empty (both quotes kept), parsed — the structural check
            // that the spans were right: a wrong span breaks the JSON
            // and the frame fails closed, the parsed path's own
            // malformed answer.
            let mut cut = Vec::with_capacity(frame.len() - span.len());
            cut.extend_from_slice(&frame[..span.start]);
            cut.extend_from_slice(&frame[span.end..]);
            let small: Value =
                serde_json::from_slice(&cut).map_err(|error| malformed(error.to_string()))?;
            (Some(span), small)
        }
        None => (
            None,
            serde_json::from_slice(&frame).map_err(|error| malformed(error.to_string()))?,
        ),
    };
    if let Some(error) = small.get("error") {
        return Err(ProxyError::Daemon(
            // The daemon's error answers always carry both fields
            // (`Response::error`); -32603 is the JSON-RPC fallback tier.
            error["code"].as_i64().unwrap_or(-32603),
            error["message"].as_str().unwrap_or_default().to_owned(),
        ));
    }
    let stdout = match span {
        Some(span) => Some(unescape_in_place(&mut frame, span).map_err(malformed)?),
        None => None,
    };
    Ok(RoutedAnswer {
        frame,
        stdout,
        small,
    })
}

/// Locates the stdout member's string body — the span between the
/// quotes — in the raw response frame. The daemon's writer spells every
/// string's quotes escaped, so the byte sequence `"stdout"` can only be
/// a member's key; a candidate that does not read `"stdout"` `:` `"`
/// (whitespace tolerated — the writer is compact) is skipped, so
/// payload text can neither forge nor derail the scan. `None` when the
/// frame carries no stdout string.
fn locate_stdout_string(frame: &[u8]) -> Option<std::ops::Range<usize>> {
    const KEY: &[u8] = b"\"stdout\"";
    let mut from = 0;
    while let Some(hit) = memchr::memmem::find(&frame[from..], KEY) {
        let key = from + hit;
        let mut at = key + KEY.len();
        let whitespace = |at: &mut usize| {
            while matches!(
                frame.get(*at),
                Some(b' ') | Some(b'\t') | Some(b'\n') | Some(b'\r')
            ) {
                *at += 1;
            }
        };
        whitespace(&mut at);
        if frame.get(at) != Some(&b':') {
            from = key + 1;
            continue;
        }
        at += 1;
        whitespace(&mut at);
        if frame.get(at) != Some(&b'"') {
            from = key + 1;
            continue;
        }
        // The value's opening quote: the body runs to the first
        // unescaped quote.
        if let Some(close) = closing_quote(frame, at) {
            return Some(at + 1..close);
        }
        from = key + 1;
    }
    None
}

/// The string's closing quote: the first quote with an even run of
/// backslashes before it (a quote behind an odd run is escaped — the
/// standard parity rule; a `\uXXXX` escape never spells a raw quote).
fn closing_quote(frame: &[u8], open: usize) -> Option<usize> {
    let mut at = open + 1;
    while let Some(hit) = memchr::memchr(b'"', &frame[at..]) {
        let quote = at + hit;
        let mut slashes = 0;
        while slashes < quote && frame[quote - 1 - slashes] == b'\\' {
            slashes += 1;
        }
        if slashes % 2 == 0 {
            return Some(quote);
        }
        at = quote + 1;
    }
    None
}

/// Unescapes one JSON string body — the span between the quotes — in
/// place and truncates the buffer to the result: the write cursor
/// starts at the read cursor and never passes it (`\"` spends two bytes
/// for one, `\uXXXX` six for at most three, a surrogate pair twelve for
/// four), so the frame's single allocation becomes the stdout buffer.
/// Verbatim runs validate as UTF-8 while they copy — the check the
/// whole-frame parse used to pay — and a malformed escape, an unpaired
/// surrogate, or invalid UTF-8 is an error (the malformed transport the
/// parsed path answers, never wrong bytes printed).
fn unescape_in_place(
    frame: &mut Vec<u8>,
    body: std::ops::Range<usize>,
) -> Result<std::ops::Range<usize>, String> {
    let (from, to) = (body.start, body.end);
    let mut read = from;
    let mut write = from;
    while read < to {
        let run_end = match memchr::memchr(b'\\', &frame[read..to]) {
            Some(hit) => read + hit,
            None => to,
        };
        // The verbatim run validates as UTF-8 as it lands (escapes only
        // ever encode whole code points, so run boundaries fall on
        // sequence boundaries).
        if std::str::from_utf8(&frame[read..run_end]).is_err() {
            return Err("invalid UTF-8 in the stdout body".to_owned());
        }
        if write != read {
            frame.copy_within(read..run_end, write);
        }
        write += run_end - read;
        if run_end == to {
            break;
        }
        // One escape at `run_end`; `skip` covers its whole spelling.
        let escape_at = run_end;
        let skip;
        match *frame.get(escape_at + 1).ok_or("a truncated escape")? {
            b'"' => {
                frame[write] = b'"';
                write += 1;
                skip = 2;
            }
            b'\\' => {
                frame[write] = b'\\';
                write += 1;
                skip = 2;
            }
            b'/' => {
                frame[write] = b'/';
                write += 1;
                skip = 2;
            }
            b'b' => {
                frame[write] = 0x08;
                write += 1;
                skip = 2;
            }
            b'f' => {
                frame[write] = 0x0C;
                write += 1;
                skip = 2;
            }
            b'n' => {
                frame[write] = 0x0A;
                write += 1;
                skip = 2;
            }
            b'r' => {
                frame[write] = 0x0D;
                write += 1;
                skip = 2;
            }
            b't' => {
                frame[write] = 0x09;
                write += 1;
                skip = 2;
            }
            b'u' => {
                let hex = |at: usize| -> Result<u16, String> {
                    let digits = frame.get(at..at + 4).ok_or("a truncated \\u escape")?;
                    let digits = std::str::from_utf8(digits)
                        .ok()
                        .and_then(|digits| u16::from_str_radix(digits, 16).ok())
                        .ok_or("an invalid \\u escape")?;
                    Ok(digits)
                };
                let code = hex(escape_at + 2)?;
                let (code_point, escape_len) = if (0xD800..=0xDBFF).contains(&code) {
                    // A leading surrogate must pair with a trailing one
                    // — the pair spells one astral code point.
                    if frame.get(escape_at + 6) != Some(&b'\\')
                        || frame.get(escape_at + 7) != Some(&b'u')
                    {
                        return Err("a lone leading surrogate in a \\u escape".to_owned());
                    }
                    let low = hex(escape_at + 8)?;
                    if !(0xDC00..=0xDFFF).contains(&low) {
                        return Err("a lone leading surrogate in a \\u escape".to_owned());
                    }
                    let code_point =
                        0x10000 + (u32::from(code) - 0xD800) * 0x400 + (u32::from(low) - 0xDC00);
                    (code_point, 12)
                } else if (0xDC00..=0xDFFF).contains(&code) {
                    return Err("a lone trailing surrogate in a \\u escape".to_owned());
                } else {
                    (u32::from(code), 6)
                };
                let decoded =
                    char::from_u32(code_point).ok_or("an unpaired surrogate in a \\u escape")?;
                // Safe by the cursor invariant: `write` trails the read
                // cursor by at least the escape's surplus.
                write += decoded.encode_utf8(&mut frame[write..write + 4]).len();
                skip = escape_len;
            }
            _ => return Err("an invalid escape".to_owned()),
        }
        read = escape_at + skip;
    }
    frame.truncate(write);
    Ok(from..write)
}

// ---------------------------------------------------------------------------
// The routing gate
// ---------------------------------------------------------------------------

/// Whether `dartq analyze`'s invocation may consult the resident engine:
/// the plain face only — the per-run engine posture flags
/// (`--overlay`, `--packages`, `--enable-experiment`, `--no-plugins`,
/// `--memory-limit`, `--memory-report`, `--trace-events`,
/// `--cache-size-limit`) re-shape the engine or its arming in ways a
/// shared resident cannot honor per request, so those run in-process
/// (`--memory-report` reads the in-process engine's own budget ladder;
/// the resident session's posture is the daemon's `dartq/memory` face).
/// `--fast` is the check pipeline and takes the check face's
/// attach-only route instead.
pub(crate) fn analyze_routable(args: &crate::AnalyzeArgs) -> bool {
    !args.fast
        && args.overlays.is_empty()
        && args.packages.is_none()
        && args.enable_experiments.is_empty()
        && !args.no_plugins
        && args.memory_limit.is_none()
        && !args.memory_report
        && args.trace_events.is_none()
        && args.cache_size_limit.is_none()
}

/// The routed analyze: elect/attach the session for the targets' one
/// workspace root (spawning and waiting when none is live — `spawn` is
/// the detached serve bring-up), ask `dartq/analyze` for the same
/// targets, and print the daemon-rendered bytes through the caller's
/// writer. `Some(code)` is the finished run (exit tier included);
/// `None` means the call did not route — the caller runs the in-process
/// path, with the fallback advisory already on stderr when a session
/// was attempted and lost.
pub(crate) fn route_analyze(
    args: &crate::AnalyzeArgs,
    targets: &[crate::TargetKind],
    out: &mut impl Write,
    spawn: &dyn Fn(&Layout, &SessionTriple) -> io::Result<()>,
    roots: &mut crate::RootMemo,
) -> Option<u8> {
    if !analyze_routable(args) {
        return None;
    }
    // The session is per workspace root; a run spanning several stays
    // in-process (one engine per root is the cold face's own shape). The
    // walks share the invocation's root memo (#4260) with the cold path's
    // grouping below.
    let targets_roots: Vec<PathBuf> = targets
        .iter()
        .map(|target| match target {
            crate::TargetKind::Dir(dir) => roots.workspace_root(dir),
            crate::TargetKind::File(file) => {
                roots.workspace_root(&file.parent().map(Path::to_path_buf).unwrap_or_default())
            }
        })
        .collect();
    let root = targets_roots.first()?.clone();
    if targets_roots.iter().any(|other| *other != root) {
        return None;
    }
    // The triple keys on the concrete SDK root: the flag, the env, or
    // the live resident session's own; without one the cold path owns
    // the usage error (its assembly resolves the same way).
    let sdk_root = sdk_root_for(&root, args.sdk_path.as_deref()).ok()?;
    let params = json!({
        "targets": targets
            .iter()
            .map(|target| match target {
                crate::TargetKind::Dir(dir) | crate::TargetKind::File(dir) => {
                    dir.display().to_string()
                }
            })
            .collect::<Vec<_>>(),
        "format": format_word(args.format),
    });
    let started = Instant::now();
    match ladder(&root, &sdk_root, "dartq/analyze", &params, spawn) {
        Ok(answer) if answer.stdout.is_some() => {
            // Belt-and-braces (#2675): the daemon invalidates the universe
            // on a package-config move before answering (the correct fix),
            // and the answer names the package config its universe was
            // assembled from — when the disk holds different bytes NOW,
            // the answer must not be printed; the cold path owns the
            // fresh universe.
            if !universe_matches_disk(&root, answer.result().get("packages")) {
                advisory("the session answered a stale package universe", "analyzing");
                return None;
            }
            Some(print_routed(
                out,
                &answer,
                args.format,
                args.memory,
                !args.no_fatal_warnings,
                args.fatal_infos,
                args.metrics_json.as_deref(),
                started,
            ))
        }
        // A same-generation daemon answering without the rendered face
        // is a divergence, not a crash: fall back to the bytes the cold
        // path owns (`session_call` defaults a missing result to `{}`).
        Ok(_) => {
            advisory("the session's answer lacked the rendered face", "analyzing");
            None
        }
        Err(reason) => {
            advisory(&reason, "analyzing");
            None
        }
    }
}

/// Whether the routed answer's analysis universe is the one the disk
/// holds now (#2675): the answer's `packages` member names the package
/// config the universe was assembled from, with the (mtime, size) it
/// was read at (`null` — the universe assembled over no config at all).
/// Current means the disk holds exactly those bytes now; any
/// disagreement — bytes moved, either side exists without the other —
/// means the answer reflects a universe the caller must not print. An
/// absent member is no signal (the check face's rendered answer reads
/// no universe and carries none).
fn universe_matches_disk(root: &Path, packages: Option<&Value>) -> bool {
    /// The universe's package config as the daemon's answer names it.
    enum Wire {
        /// No config anywhere at assembly (the BasicWorkspace floor).
        None,
        /// The named path, with the (mtime_ns, size) it was read at.
        Stamped {
            path: PathBuf,
            mtime_ns: u64,
            size: u64,
        },
    }
    let named = match packages {
        // An absent member is no signal (the check face's rendered
        // answer reads no universe and carries none).
        None => return true,
        // JSON `null`: the universe assembled over no config at all.
        Some(named) => named,
    };
    let wire = match named.as_object() {
        None => Wire::None,
        Some(named) => match (
            named["path"].as_str(),
            named["mtime_ns"].as_u64(),
            named["size"].as_u64(),
        ) {
            (Some(path), Some(mtime_ns), Some(size)) => Wire::Stamped {
                path: PathBuf::from(path),
                mtime_ns,
                size,
            },
            _ => Wire::None,
        },
    };
    let conventional = root.join(".dart_tool/package_config.json");
    let disk = std::fs::metadata(match &wire {
        Wire::Stamped { path, .. } => path,
        Wire::None => &conventional,
    })
    .ok()
    .map(|meta| {
        let mtime_ns = meta
            .modified()
            .ok()
            .and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|since| u64::try_from(since.as_nanos()).unwrap_or(u64::MAX))
            .unwrap_or(0);
        (mtime_ns, meta.len())
    });
    match (wire, disk) {
        // No universe and no config now: current.
        (Wire::None, None) => true,
        // One side holds a config the other lacks: stale.
        (Wire::None, Some(_)) | (Wire::Stamped { .. }, None) => false,
        (Wire::Stamped { mtime_ns, size, .. }, Some((disk_mtime_ns, disk_size))) => {
            mtime_ns == disk_mtime_ns && size == disk_size
        }
    }
}

/// The routed check: consult a live session for the first target's
/// workspace root — attach only, never spawning (the M0 pipeline has no
/// resident state; a spawn would pay the engine bring-up a syntax sweep
/// must not). `Some(code)` finishes the run; `None` falls to the
/// in-process pipeline (the fallback advisory on stderr when a session
/// was found but lost).
pub(crate) fn route_check(args: &crate::CheckArgs, out: &mut impl Write) -> Option<u8> {
    // Every path must resolve — the cold face owns the missing-target
    // usage error, so a path that cannot canonicalize here routes never.
    let paths: Vec<PathBuf> = args
        .paths
        .iter()
        .map(|path| path.canonicalize().ok())
        .collect::<Option<Vec<_>>>()?;
    let root = crate::workspace_root_of(&paths[0]);
    let socket = attached_socket(&root)?;
    let format = check_format(args);
    let params = json!({
        "paths": paths
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>(),
        "format": format_word(format),
    });
    let started = Instant::now();
    match session_call_routed(&socket, "dartq/check", &params, CALL_TIMEOUT) {
        Ok(answer) if answer.stdout.is_some() => Some(print_routed(
            out,
            &answer,
            format,
            false,
            !args.no_fatal_warnings,
            args.fatal_infos,
            args.metrics_json.as_deref(),
            started,
        )),
        Ok(_) => {
            advisory("the session's answer lacked the rendered face", "analyzing");
            None
        }
        Err(error) => {
            advisory(&error.reason(), "analyzing");
            None
        }
    }
}

// ---------------------------------------------------------------------------
// The answer faces' routes (#2629): query (every kind) + batch, explain,
// impact, rename (dry run), resolve, complete — attach-only, the check
// face's doctrine (see the module docs)
// ---------------------------------------------------------------------------

/// One engine-view answer face's route: attach a live session for the
/// face's workspace root — under the FULL triple, so the session's engine
/// is the one the cold path would assemble — send `method` the face's
/// params, and print the daemon-rendered stdout bytes. `Some(code)` finishes the run (exit 0 —
/// the value faces' tier on an answered document); `None` falls back to
/// the in-process path — silently when nothing is live (the normal
/// shape), with the fallback advisory on stderr when a session was found
/// and lost. `stamped` faces carry the answer's universe stamp and verify
/// it against the disk exactly as analyze's does; `explain`'s answer
/// carries none (its per-query engine reads the disk fresh).
fn route_answer(
    root: &Path,
    sdk_path: Option<&Path>,
    method: &str,
    params: &Value,
    out: &mut impl Write,
) -> Option<u8> {
    // The triple keys on the concrete SDK root: the flag, the env, or
    // the live resident session's own; without one the cold path owns
    // the usage error (its assembly resolves the same way).
    let sdk_root = sdk_root_for(root, sdk_path).ok()?;
    let socket = attached_socket_for(root, &sdk_root)?;
    match session_call_routed(&socket, method, params, CALL_TIMEOUT) {
        Ok(answer) if answer.stdout.is_some() => {
            if !universe_matches_disk(root, answer.result().get("packages")) {
                advisory("the session answered a stale package universe", ANSWERING);
                return None;
            }
            print_answer(out, &answer)
        }
        Ok(_) => {
            advisory("the session's answer lacked the rendered face", ANSWERING);
            None
        }
        Err(error) => {
            advisory(&error.reason(), ANSWERING);
            None
        }
    }
}

/// The attach-only route over an already-probed socket (explain's shape —
/// no SDK triple, no universe stamp: the per-query engine reads the disk
/// fresh).
fn route_attached(socket: &Path, method: &str, params: &Value, out: &mut impl Write) -> Option<u8> {
    match session_call_routed(socket, method, params, CALL_TIMEOUT) {
        Ok(answer) if answer.stdout.is_some() => print_answer(out, &answer),
        Ok(_) => {
            advisory("the session's answer lacked the rendered face", ANSWERING);
            None
        }
        Err(error) => {
            advisory(&error.reason(), ANSWERING);
            None
        }
    }
}

/// Prints one rendered answer's bytes and finishes the run: the value
/// faces exit 0 on an answered document — their usage errors are the cold
/// path's own, re-derived there whenever the route falls back.
fn print_answer(out: &mut impl Write, answer: &RoutedAnswer) -> Option<u8> {
    // The answer shape is this binary generation's own (the attach probe
    // pins the protocol generation by binary stamp): a missing field is a
    // program bug, not a runtime case.
    let stdout = answer.stdout();
    if out.write_all(stdout).and_then(|()| out.flush()).is_err() {
        eprintln!("dartq: internal error: writing the answer failed");
        return Some(exit::INTERNAL_ERROR);
    }
    Some(exit::CLEAN)
}

/// The answer faces' fallback advisory tail (analyze/check keep their own
/// "analyzing" spelling).
const ANSWERING: &str = "answering";

/// The face's package directory as the canonical resolution base: the
/// same base `open_engine` resolves against below. `None` (an
/// unresolvable directory) routes never — the cold path owns the usage
/// error.
fn face_root(dir: &Path) -> Option<(PathBuf, PathBuf)> {
    let dir = dir.canonicalize().ok()?;
    Some((crate::workspace_root_of(&dir), dir))
}

/// `dartq query <face> NAME` over one kind: the daemon's `dartq/query`
/// answers the same tier-shaped envelope the cold face prints. The
/// `fuzzy` opt-in rides only when set — the standing request params stay
/// byte-identical, and the CLI refuses the flag on the reference-family
/// faces before this route runs.
pub(crate) fn route_query(
    args: &crate::SymbolArgs,
    kind: crate::QueryKind,
    out: &mut impl Write,
) -> Option<u8> {
    let (root, _dir) = face_root(&args.dir)?;
    let mut params = json!({
        "name": args.name,
        "kind": query_kind_word(kind),
        "format": output_word(args.format),
        "verbosity": verbosity_word(args.verbosity),
    });
    if args.fuzzy {
        params["fuzzy"] = json!(true);
    }
    route_answer(&root, args.sdk.as_deref(), "dartq/query", &params, out)
}

/// `dartq query batch`: the daemon's `dartq/queryBatch` answers every
/// item in one round trip over the session graph, rendered as the cold
/// face prints them — one line per envelope, request order.
pub(crate) fn route_query_batch(args: &crate::BatchArgs, out: &mut impl Write) -> Option<u8> {
    let (root, _dir) = face_root(&args.dir)?;
    let kinds: Vec<&str> = if args.kinds.is_empty() {
        vec!["symbol"]
    } else {
        args.kinds
            .iter()
            .map(|kind| query_kind_word(*kind))
            .collect()
    };
    let queries: Vec<Value> = args
        .symbols
        .iter()
        .map(|name| json!({"name": name, "kinds": kinds}))
        .collect();
    let params = json!({
        "queries": queries,
        "format": output_word(args.format),
        "verbosity": verbosity_word(args.verbosity),
    });
    route_answer(&root, args.sdk.as_deref(), "dartq/queryBatch", &params, out)
}

/// `dartq impact`: the daemon's `dartq/impact` over the same target and
/// the same `--symbol` narrowing list (#2666).
pub(crate) fn route_impact(args: &crate::impact::ImpactArgs, out: &mut impl Write) -> Option<u8> {
    let (root, dir) = face_root(&args.dir)?;
    // The same resolution base the cold face resolves against (the
    // assembly's canonical package directory): a relative target joins it,
    // an absolute one passes through (the core canonicalizes either way).
    let target = if args.path.is_absolute() {
        args.path.clone()
    } else {
        dir.join(&args.path)
    };
    let params = json!({
        "path": target.display().to_string(),
        "symbols": args.symbols,
        "format": output_word(args.format),
        "verbosity": verbosity_word(args.verbosity),
    });
    route_answer(&root, args.sdk.as_deref(), "dartq/impact", &params, out)
}

/// `dartq explain` (the symbol face; the `--diagnostic` face is a
/// different query with no daemon spelling and stays cold).
pub(crate) fn route_explain(
    args: &crate::ExplainArgs,
    symbol: &str,
    out: &mut impl Write,
) -> Option<u8> {
    let (root, _dir) = face_root(&args.dir)?;
    let mut params = json!({
        "symbol": symbol,
        "format": output_word(args.format),
        "verbosity": verbosity_word(args.verbosity),
    });
    if let Some(sdk) = &args.sdk {
        params["sdk"] = json!(sdk.display().to_string());
    }
    // No universe stamp, and the plain attach probe (the SDK-free one):
    // the daemon's explain rebuilds a per-query engine over the on-disk
    // base under the request's own `sdk`, so its bytes read the disk
    // fresh and never the session's engine.
    let socket = attached_socket(&root)?;
    route_attached(&socket, "dartq/explain", &params, out)
}

/// `dartq rename` — the dry run only (`--write` routes never: the write
/// applies through the CLI's engine-local splice machinery): the daemon's
/// `dartq/rename` answers the same edit-set document over the analyzed
/// view. The anchor rides the wire as the user spelled it; the daemon
/// converts against the analyzed text exactly where the cold face
/// converts against its own.
pub(crate) fn route_rename(
    args: &crate::rename::RenameArgs,
    target: &Path,
    anchor: dartq_commands::rename::Anchor,
    out: &mut impl Write,
) -> Option<u8> {
    let (root, _dir) = face_root(&args.dir)?;
    let mut params = json!({
        "path": target.display().to_string(),
        "new_name": args.new_name,
        "format": output_word(args.format),
    });
    match anchor {
        dartq_commands::rename::Anchor::LineCol { line, column } => {
            params["line"] = json!(line);
            params["col"] = json!(column);
        }
        dartq_commands::rename::Anchor::Utf16Offset(offset) => {
            params["offset"] = json!(offset);
        }
    }
    route_answer(&root, args.sdk.as_deref(), "dartq/rename", &params, out)
}

/// `dartq resolve` / `dartq complete`: the daemon's position faces answer
/// the same envelope; the cursor rides the wire in the line/col spelling
/// and converts against the analyzed view's own text (the same
/// conversion, the same snapshot, as the cold face's).
pub(crate) fn route_position(
    args: &crate::resolve::PositionArgs,
    method: &str,
    file: &Path,
    line: u32,
    column: u32,
    out: &mut impl Write,
) -> Option<u8> {
    let (root, _dir) = face_root(&args.dir)?;
    let params = json!({
        "file": file.display().to_string(),
        "line": line,
        "col": column,
        "format": output_word(args.format),
    });
    route_answer(&root, args.sdk.as_deref(), method, &params, out)
}

/// The CLI output format's wire word (the daemon `format` parameter's
/// spellings).
fn output_word(format: crate::OutputFormat) -> &'static str {
    match format {
        crate::OutputFormat::Lon => "lon",
        crate::OutputFormat::Json => "json",
        crate::OutputFormat::Text => "text",
    }
}

/// The CLI verbosity tier's wire word.
fn verbosity_word(verbosity: crate::Verbosity) -> &'static str {
    match verbosity {
        crate::Verbosity::Minimal => "minimal",
        crate::Verbosity::Compact => "compact",
        crate::Verbosity::Full => "full",
    }
}

/// One query kind's wire word (the daemon `kind` parameter's spellings).
fn query_kind_word(kind: crate::QueryKind) -> &'static str {
    match kind {
        crate::QueryKind::Symbol => "symbol",
        crate::QueryKind::References => "references",
        crate::QueryKind::Callers => "callers",
        crate::QueryKind::Implementations => "implementations",
        crate::QueryKind::Dependencies => "dependencies",
    }
}

/// The routing ladder: ensure (elect/attach/spawn), call, and on any
/// failure re-elect once — the election reaps stale generations and
/// respawns — before giving the reason up. A session that answers is
/// the routed answer; anything else is the caller's cold fallback.
fn ladder(
    root: &Path,
    sdk_root: &Path,
    method: &str,
    params: &Value,
    spawn: &dyn Fn(&Layout, &SessionTriple) -> io::Result<()>,
) -> Result<RoutedAnswer, String> {
    let call = |layout: &Layout| session_call_routed(&layout.socket, method, params, CALL_TIMEOUT);
    let layout = Layout::new(root);
    match ensure(root, sdk_root, spawn) {
        Ensured::Attached(_) => match call(&layout) {
            Ok(answer) => Ok(answer),
            Err(first) => match ensure(root, sdk_root, spawn) {
                Ensured::Attached(_) => call(&layout).map_err(|second| {
                    format!(
                        "{}, then re-election failed: {}",
                        first.reason(),
                        second.reason()
                    )
                }),
                Ensured::Standalone(reason) => {
                    Err(format!("{}, then re-election: {reason}", first.reason()))
                }
            },
        },
        Ensured::Standalone(reason) => Err(reason),
    }
}

/// The attach-only probe (the check face): the layout's live state —
/// the same resolution root, the same binary generation, live pid,
/// fresh heartbeat, answering socket — names the session's socket, with
/// nothing written and nothing spawned. The check pipeline is
/// SDK-free, so the session identity it consults is root + binary stamp
/// (a daemon raised under another SDK serves the same M0 bytes); the
/// analyze face's full-triple match lives in the election.
fn attached_socket(root: &Path) -> Option<PathBuf> {
    let layout = Layout::new(root);
    let state = SessionState::read(&layout)?;
    let fresh =
        SessionState::now_ms().saturating_sub(state.heartbeat_ms) <= STALE_HEARTBEAT.as_millis();
    // The dummy SDK root never touches the comparison: only the triple's
    // root and binary stamp are read (the stamp is the current exe's own).
    let here = SessionTriple::compute(root, root);
    (state.triple.root == here.root
        && state.triple.binary_stamp == here.binary_stamp
        && state.pid != 0
        && dartq_session::pid_alive(state.pid)
        && fresh
        && dartq_session::socket_open(&layout))
    .then(|| layout.socket.clone())
}

/// The engine-view answer faces' attach probe (#2629):
/// [`attached_socket`]'s checks with the FULL session triple — the SDK
/// root included. These answers read the session's semantic engine: a
/// daemon raised under another SDK would answer bytes the cold path
/// (assembling under this SDK) never prints, so a foreign-SDK session is
/// no session for these faces — the cold path owns the answer.
fn attached_socket_for(root: &Path, sdk_root: &Path) -> Option<PathBuf> {
    let layout = Layout::new(root);
    let state = SessionState::read(&layout)?;
    let fresh =
        SessionState::now_ms().saturating_sub(state.heartbeat_ms) <= STALE_HEARTBEAT.as_millis();
    let here = SessionTriple::compute(root, sdk_root);
    (state.triple == here
        && state.pid != 0
        && dartq_session::pid_alive(state.pid)
        && fresh
        && dartq_session::socket_open(&layout))
    .then(|| layout.socket.clone())
}

/// The one-line fallback advisory (the ticket's wording; `doing` is the
/// face's own gerund — "analyzing" for the diagnostics faces, "answering"
/// for the value faces).
fn advisory(reason: &str, doing: &str) {
    eprintln!("dartq: resident session unavailable ({reason}); {doing} in-process");
}

// ---------------------------------------------------------------------------
// The routed postlude
// ---------------------------------------------------------------------------

/// The daemon's format word for a CLI analyze format.
fn format_word(format: crate::AnalyzeFormat) -> &'static str {
    match format {
        crate::AnalyzeFormat::Lon => "lon",
        crate::AnalyzeFormat::Json => "json",
        crate::AnalyzeFormat::Text => "text",
    }
}

/// The check face's formats ride the same three spellings.
fn check_format(args: &crate::CheckArgs) -> crate::AnalyzeFormat {
    match args.format {
        crate::CheckFormat::Lon => crate::AnalyzeFormat::Lon,
        crate::CheckFormat::Json => crate::AnalyzeFormat::Json,
        crate::CheckFormat::Text => crate::AnalyzeFormat::Text,
    }
}

/// Prints one routed answer and finishes the run: the workspace advisory
/// on the stream the cold face uses (stderr for the data faces, the
/// leading stdout line for text), the daemon-rendered bytes verbatim
/// (#4190: the spliced stdout, written straight from the frame buffer —
/// the `--memory` splice is the one face that edits the tail, and only
/// it pays a copy), the `--metrics-json` facts (the routed phases:
/// `analyze_ms` is the round trip, `render_ms` the write; the cold
/// path's discovery/sdk/assembly/lints/library numbers have no routed
/// counterpart and report 0), and the severity exit tier over the
/// answer's counts.
fn print_routed(
    out: &mut impl Write,
    answer: &RoutedAnswer,
    format: crate::AnalyzeFormat,
    memory_key: bool,
    fatal_warnings: bool,
    fatal_infos: bool,
    metrics_path: Option<&Path>,
    started: Instant,
) -> u8 {
    // The advisory rides the answer, never the bytes: the cold face
    // prints it on stderr for the data formats and leads stdout with it
    // on the text face.
    if let Some(advisory) = answer.result().get("advisory").and_then(Value::as_object) {
        let line = format!(
            "Info • {} • {}",
            advisory["code"].as_str().unwrap_or_default(),
            advisory["message"].as_str().unwrap_or_default(),
        );
        match format {
            crate::AnalyzeFormat::Text => {
                let _ = writeln!(out, "{line}");
            }
            crate::AnalyzeFormat::Lon | crate::AnalyzeFormat::Json => {
                eprintln!("{line}");
            }
        }
    }
    // The answer shape is this binary generation's own (the election
    // pins the protocol generation by binary stamp): a missing field is a
    // program bug, not a runtime case.
    let stdout = answer.stdout();
    let render_at = Instant::now();
    // The oracle's hidden `--memory` key (analyze face, json only): the
    // same splice the cold face applies — the peak RSS reported is the
    // calling process's own, the quantity the flag names.
    let written = if memory_key && format == crate::AnalyzeFormat::Json {
        let mut line = String::from_utf8(stdout.to_vec()).expect("the writers emit UTF-8");
        crate::splice_memory_key(&mut line);
        out.write_all(line.as_bytes())
    } else {
        out.write_all(stdout)
    }
    .and_then(|()| out.flush());
    let render_ms = u64::try_from(render_at.elapsed().as_millis()).unwrap_or(u64::MAX);
    if let Err(error) = written {
        eprintln!("dartq: internal error: writing diagnostics failed: {error}");
        return exit::INTERNAL_ERROR;
    }
    if let Some(path) = metrics_path {
        let counts = answer.result().get("counts").unwrap_or(&Value::Null);
        let diagnostics = counts["error"].as_u64().unwrap_or(0)
            + counts["warning"].as_u64().unwrap_or(0)
            + counts["info"].as_u64().unwrap_or(0);
        let total = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let metrics = crate::RunMetrics {
            files: 0,
            loc: 0,
            diagnostics: usize::try_from(diagnostics).unwrap_or(usize::MAX),
            discovery_ms: 0,
            sdk_ms: 0,
            assembly_ms: 0,
            analyze_ms: total,
            lints_ms: 0,
            render_ms,
            total_ms: total,
            libraries_analyzed: 0,
            libraries_from_cache: 0,
            max_rss_bytes: crate::max_rss_bytes(),
        };
        if let Err(error) = crate::write_metrics(path, &metrics) {
            eprintln!("dartq: internal error: writing metrics failed: {error}");
            return exit::INTERNAL_ERROR;
        }
    }
    counts_exit(
        answer.result().get("counts").unwrap_or(&Value::Null),
        fatal_warnings,
        fatal_infos,
    )
}

/// The routed exit tier: the severity arithmetic over the answer's
/// counts — errors always fatal, warnings under `--fatal-warnings`,
/// infos under `--fatal-infos` (the cold path's own ladder).
fn counts_exit(counts: &Value, fatal_warnings: bool, fatal_infos: bool) -> u8 {
    let count = |word: &str| counts[word].as_u64().unwrap_or(0) > 0;
    crate::severity_tier(
        count("error"),
        count("warning"),
        count("info"),
        fatal_warnings,
        fatal_infos,
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// The routing tests share the signal-serialization lock with the serve
/// tests (see `session::tests`): every serve loop here is collateral of a
/// stray SIGTERM otherwise.
#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::RootMemo;
    use crate::session::tests::SIGNAL_SERIALIZATION;
    use crate::{AnalyzeFormat, TargetKind};

    /// One fixture with a broken library — rows on the parser and the
    /// semantic tiers — over the CLI analyze tests' own tree shape.
    fn broken_fixture(tag: &str) -> PathBuf {
        crate::tests::analyze_fixture(
            tag,
            &[
                ("lib/broken.dart", "void f() { x }\n"),
                ("lib/clean.dart", "var ok = 1;\n"),
            ],
        )
    }

    /// The routed variant of the test args: the cold helper with the
    /// consult armed.
    fn routed_args(root: &Path, format: AnalyzeFormat) -> crate::AnalyzeArgs {
        let mut args = crate::tests::analyze_args(root, format, None);
        args.cold = false;
        args
    }

    /// A serve loop over `root` with an idle window long enough to cover
    /// a whole test (the 1 s self-reap of `serve_loop_for_tests` races
    /// the cold passes run beside it); polls until the socket listens.
    fn serve(root: &Path) {
        let dir = root.to_path_buf();
        let sdk = root.join("sdk");
        std::thread::spawn(move || {
            crate::session::serve_loop_for_tests_with(&dir, &sdk, Duration::from_secs(120))
        });
        let layout = Layout::new(root);
        let deadline = Instant::now() + Duration::from_secs(30);
        while !dartq_session::socket_open(&layout) {
            assert!(Instant::now() < deadline, "the serve loop never listened");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// A spawn closure that refuses — the attach path never reaches it,
    /// and the ladder test's bring-up failure runs on it.
    fn refused_spawn() -> impl Fn(&Layout, &SessionTriple) -> io::Result<()> {
        move |_, _| Err(io::Error::other("bring-up refused by the test"))
    }

    /// Ends the fixture's session and removes the tree.
    fn teardown(root: &Path) {
        if let Some(state) = SessionState::read(&Layout::new(root)) {
            dartq_session::terminate(state.pid);
            for _ in 0..20 {
                if !dartq_session::pid_alive(state.pid) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        dartq_session::Registry::open().remove(root);
        std::fs::remove_dir_all(root).ok();
    }

    /// The routing gate: every per-run posture flag and the `--fast`
    /// spelling stay in-process; the plain face routes. Pure and fast —
    /// no fixture, no socket.
    #[test]
    fn the_gate_skips_what_the_resident_cannot_serve() {
        let plain = routed_args(Path::new("/w"), AnalyzeFormat::Lon);
        assert!(analyze_routable(&plain), "the plain face routes");
        let skews: [(&str, Box<dyn Fn(&mut crate::AnalyzeArgs)>); 8] = [
            ("--fast", Box::new(|args| args.fast = true)),
            (
                "--overlay",
                Box::new(|args| args.overlays = vec!["a=b".into()]),
            ),
            (
                "--packages",
                Box::new(|args| args.packages = Some(PathBuf::from("p"))),
            ),
            (
                "--enable-experiment",
                Box::new(|args| args.enable_experiments = vec!["x".into()]),
            ),
            ("--no-plugins", Box::new(|args| args.no_plugins = true)),
            (
                "--memory-limit",
                Box::new(|args| args.memory_limit = Some(64)),
            ),
            (
                "--trace-events",
                Box::new(|args| args.trace_events = Some(PathBuf::from("t"))),
            ),
            (
                "--cache-size-limit",
                Box::new(|args| args.cache_size_limit = Some(1024)),
            ),
        ];
        for (flag, skew) in skews {
            let mut args = routed_args(Path::new("/w"), AnalyzeFormat::Lon);
            skew(&mut args);
            assert!(!analyze_routable(&args), "{flag} stays in-process");
        }
    }

    /// The splice's exactness pin (#4190): the stdout extractor's
    /// unescape is byte-exact over every escape class — the ones the
    /// daemon's writer emits (named escapes, control `\uXXXX`, raw
    /// multibyte UTF-8) and the ones only a foreign writer might (slash,
    /// backspace, form feed, `\uXXXX` surrogate pairs) — against the
    /// serde-parsed equivalent of the very same frame, with the decoy
    /// spellings (`\"stdout\":`, `\"counts\":`) the payload could try to
    /// forge a member with.
    #[test]
    fn the_stdout_splice_unescapes_every_escape_class_exactly() {
        // The writer-realistic frame: the payload serialized by the
        // daemon escaper's own serde machinery, members after stdout
        // kept (the splice cuts the member, never the tail).
        let payload = concat!(
            "quote:\" backslash:\\ newline:\n tab:\t cr:\r bell:\u{7} esc:\u{1b} ",
            "del:\u{7f} nbsp:λ cjk:漢 emoji:\u{1F600} math:\u{1D54F} ",
            "decoys:\"stdout\": \"counts\": \"packages\": done",
        );
        let frame = format!(
            concat!(
                r#"{{"jsonrpc":"2.0","id":1,"result":{{"v":1,"stdout":"#,
                r#"{},"counts":{{"error":1,"warning":2,"info":3}},"#,
                r#""advisory":null,"packages":null}}}}"#,
            ),
            serde_json::to_string(payload).expect("the payload serializes"),
        );
        let answer = routed_answer(frame.clone().into_bytes()).expect("the frame cuts");
        assert_eq!(
            answer.stdout(),
            payload.as_bytes(),
            "the splice unescapes the writer's classes exactly"
        );
        let parsed: Value = serde_json::from_str(&frame).expect("the frame parses");
        assert_eq!(
            parsed["result"]["stdout"].as_str().expect("stdout"),
            payload,
            "the serde-parsed equivalent agrees"
        );
        assert_eq!(answer.result()["counts"]["info"], json!(3));
        assert!(answer.result()["packages"].is_null());

        // The foreign-writer frame: hand-spelled escapes serde's writer
        // never emits but the decoder must answer exactly as serde's
        // parser does.
        let hand = concat!(
            r#"{"jsonrpc":"2.0","id":1,"result":{"v":1,"stdout":"#,
            r#""slash:\/ bs:\b ff:\f alpha:\u0041iao emoji:\ud83d\ude00 "#,
            r#"math:\ud835\udd4f empty:\u0000""#,
            r#","packages":{"path":"/w/.dart_tool/package_config.json","mtime_ns":1,"size":2}}}"#,
        );
        let expected = concat!(
            "slash:/ bs:\u{8} ff:\u{c} alpha:Aiao emoji:\u{1F600} ",
            "math:\u{1D54F} empty:\u{0}",
        );
        let answer = routed_answer(hand.as_bytes().to_vec()).expect("the hand frame cuts");
        assert_eq!(
            answer.stdout(),
            expected.as_bytes(),
            "the splice unescapes the foreign classes exactly"
        );
        let parsed: Value = serde_json::from_str(hand).expect("the hand frame parses");
        assert_eq!(
            parsed["result"]["stdout"].as_str().expect("stdout"),
            expected,
            "the serde-parsed equivalent agrees"
        );
        // The stamp rides the small members (the universe gate's input).
        assert_eq!(answer.result()["packages"]["mtime_ns"], json!(1u64));
    }

    /// The raw arm mapping (#4190): notifications read past by prefix,
    /// an error answer maps to the daemon error, a stdout-less result
    /// is the fallback arm, an empty stdout is a present (empty) face,
    /// and malformed frames — a bad escape, a lone surrogate, a
    /// truncated string — refuse as transport errors, never print
    /// wrong bytes.
    #[test]
    fn the_raw_arm_mapping_is_the_parsed_paths_semantics() {
        assert!(is_notification(
            br#"{"jsonrpc":"2.0","method":"dartq/diagnostics","params":{}}"#
        ));
        assert!(!is_notification(
            br#"{"jsonrpc":"2.0","id":1,"result":{"v":1,"stdout":""}}"#
        ));
        // The error answer: the daemon error, code and message.
        let error = routed_answer(
            br#"{"jsonrpc":"2.0","id":1,"error":{"code":-32000,"message":"boom"}}"#.to_vec(),
        )
        .expect_err("the error frame maps to the daemon error");
        assert!(
            matches!(error, ProxyError::Daemon(-32000, ref message) if message == "boom"),
            "{error:?}"
        );
        // A stdout-less result: no rendered face (the caller's fallback
        // arm), the small members still reading.
        let bare = routed_answer(br#"{"jsonrpc":"2.0","id":1,"result":{"v":1}}"#.to_vec())
            .expect("the bare result cuts");
        assert!(bare.stdout.is_none(), "no rendered face");
        assert_eq!(bare.result()["v"], json!(1));
        // An empty stdout is a present, empty face.
        let empty =
            routed_answer(br#"{"jsonrpc":"2.0","id":1,"result":{"v":1,"stdout":""}}"#.to_vec())
                .expect("the empty stdout cuts");
        assert_eq!(empty.stdout(), b"", "the empty face prints nothing");
        // A malformed escape refuses (the parsed path's malformed
        // transport, never wrong bytes).
        let malformed_frames: [&[u8]; 3] = [
            br#"{"jsonrpc":"2.0","id":1,"result":{"v":1,"stdout":"a\x2b","counts":null}}"#,
            br#"{"jsonrpc":"2.0","id":1,"result":{"v":1,"stdout":"lone:\ud835"}}"#,
            br#"{"jsonrpc":"2.0","id":1,"result":{"v":1,"stdout":"trunc"#,
        ];
        for frame in malformed_frames {
            let refused = routed_answer(frame.to_vec()).expect_err("the malformed frame refuses");
            assert!(matches!(refused, ProxyError::Transport(_)), "{refused:?}");
        }
    }

    /// The money pin (#2627): the routed answer's stdout is the cold
    /// path's stdout, byte for byte, on every format — the daemon renders
    /// through the same writers the cold face prints with — and the exit
    /// tier matches.
    #[test]
    fn routed_analyze_matches_the_cold_bytes_on_every_format() {
        let _serial = SIGNAL_SERIALIZATION
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        for format in [AnalyzeFormat::Lon, AnalyzeFormat::Json, AnalyzeFormat::Text] {
            let root = broken_fixture("route-bytes");
            serve(&root);
            let targets = [TargetKind::Dir(root.clone())];

            let mut cold: Vec<u8> = Vec::new();
            let cold_code =
                crate::analyze_run(&crate::tests::analyze_args(&root, format, None), &mut cold);

            let mut routed: Vec<u8> = Vec::new();
            let code = route_analyze(
                &routed_args(&root, format),
                &targets,
                &mut routed,
                &refused_spawn(),
                &mut RootMemo::default(),
            )
            .expect("the live session answers");
            assert_eq!(
                code,
                cold_code,
                "the exit tier matches ({})",
                format_word(format)
            );
            assert_eq!(
                String::from_utf8_lossy(&routed),
                String::from_utf8_lossy(&cold),
                "routed stdout == cold stdout, byte for byte ({})",
                format_word(format)
            );
            assert!(
                !routed.is_empty(),
                "the broken fixture keeps the bytes honest"
            );
            teardown(&root);
        }
    }

    /// The routed face through the spawn path: no live session, the
    /// ladder's bring-up runs the injected spawn, waits for the
    /// heartbeat, and the answer is still the cold bytes.
    #[test]
    fn the_ladder_spawns_and_waits_when_nothing_is_live() {
        let _serial = SIGNAL_SERIALIZATION
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let root = broken_fixture("route-spawn");
        let spawn_root = root.clone();
        let spawn = move |_: &Layout, _: &SessionTriple| {
            let dir = spawn_root.clone();
            let sdk = spawn_root.join("sdk");
            std::thread::spawn(move || {
                crate::session::serve_loop_for_tests_with(&dir, &sdk, Duration::from_secs(30))
            });
            Ok(())
        };
        let mut cold: Vec<u8> = Vec::new();
        let cold_code = crate::analyze_run(
            &crate::tests::analyze_args(&root, AnalyzeFormat::Json, None),
            &mut cold,
        );
        let mut routed: Vec<u8> = Vec::new();
        let code = route_analyze(
            &routed_args(&root, AnalyzeFormat::Json),
            &[TargetKind::Dir(root.clone())],
            &mut routed,
            &spawn,
            &mut RootMemo::default(),
        )
        .expect("the spawned session answers");
        assert_eq!(code, cold_code);
        assert_eq!(
            String::from_utf8_lossy(&routed),
            String::from_utf8_lossy(&cold)
        );
        teardown(&root);
    }

    /// The fallback arm: a bring-up that cannot happen (the refused
    /// spawn, nothing live) routes never and says why — the cold path
    /// still answers behind it.
    #[test]
    fn a_failed_bringup_falls_back_without_an_answer() {
        let root = broken_fixture("route-fallback");
        let outcome = route_analyze(
            &routed_args(&root, AnalyzeFormat::Lon),
            &[TargetKind::Dir(root.clone())],
            &mut Vec::new(),
            &refused_spawn(),
            &mut RootMemo::default(),
        );
        assert!(outcome.is_none(), "the failed ladder falls back cold");
        std::fs::remove_dir_all(&root).ok();
    }

    /// Freshness (#2627's hard requirement): the routed answer reflects
    /// the on-disk tree at call time — an out-of-band edit (no sync
    /// notification) lands in the very next routed call, and the revert
    /// lands after it.
    #[test]
    fn a_routed_call_reconciles_out_of_band_edits() {
        let _serial = SIGNAL_SERIALIZATION
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let root = broken_fixture("route-fresh");
        serve(&root);
        let targets = [TargetKind::Dir(root.clone())];
        let args = routed_args(&root, AnalyzeFormat::Lon);
        let call = |out: &mut Vec<u8>| {
            route_analyze(
                &args,
                &targets,
                out,
                &refused_spawn(),
                &mut RootMemo::default(),
            )
            .expect("the live session answers")
        };

        let mut before: Vec<u8> = Vec::new();
        call(&mut before);
        let text = String::from_utf8(before).expect("utf-8");
        let doc: serde_json::Value = lon::from_str(text.trim_end()).expect("one lon document");
        assert!(
            !doc["diags"].as_array().expect("rows").is_empty(),
            "the broken fixture starts dirty"
        );

        // The edit heals the tree; no notification rides — the next plain
        // request must reconcile against the disk.
        std::fs::write(root.join("lib/broken.dart"), "void f() {}\n").expect("edit");
        let mut healed: Vec<u8> = Vec::new();
        call(&mut healed);
        assert!(
            String::from_utf8_lossy(&healed).contains("diags:[]"),
            "the edit lands in the next routed answer: {}",
            String::from_utf8_lossy(&healed)
        );

        // The revert comes back the same way.
        std::fs::write(root.join("lib/broken.dart"), "void f() { x }\n").expect("revert");
        let mut broken: Vec<u8> = Vec::new();
        call(&mut broken);
        assert!(
            !String::from_utf8_lossy(&broken).contains("diags:[]"),
            "the revert lands too: {}",
            String::from_utf8_lossy(&broken)
        );
        teardown(&root);
    }

    /// The pub-get half of #2675: the routed answer over a live session
    /// reflects a package-config change made BETWEEN calls — the
    /// once-dangling `package:dep/extra.dart` import answers at the error
    /// tier before the dependency exists and clean (rc 0, the same tier
    /// the cold face answers) after the config names it, with the session
    /// never restarting.
    #[test]
    fn a_routed_call_answers_over_a_changed_package_universe() {
        let _serial = SIGNAL_SERIALIZATION
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let root = crate::tests::analyze_fixture(
            "route-universe",
            &[(
                "lib/main.dart",
                "import 'package:dep/extra.dart';\n\nint main() => extra();\n",
            )],
        );
        serve(&root);
        let targets = [TargetKind::Dir(root.clone())];
        let args = routed_args(&root, AnalyzeFormat::Lon);

        let mut before: Vec<u8> = Vec::new();
        let code = route_analyze(
            &args,
            &targets,
            &mut before,
            &refused_spawn(),
            &mut RootMemo::default(),
        )
        .expect("the live session answers");
        assert_eq!(
            code, 3,
            "the dangling import is an error before the dependency exists"
        );
        assert!(String::from_utf8_lossy(&before).contains("uri_does_not_exist"));

        // The dependency joins the resolution (a pub get's shape: the
        // config names it, the package lands beside the workspace).
        let dep = std::env::temp_dir().join("dq_cli_analyze_route_universe_dep");
        std::fs::create_dir_all(dep.join("lib")).expect("the dep lib dir");
        std::fs::write(
            dep.join("pubspec.yaml"),
            "name: dep\nenvironment:\n  sdk: ^3.5.0\n",
        )
        .expect("the dep pubspec");
        std::fs::write(dep.join("lib/extra.dart"), "int extra() => 1;\n").expect("the dep lib");
        let config = root.join(".dart_tool/package_config.json");
        std::fs::write(
            &config,
            concat!(
                r#"{"configVersion": 2, "packages": ["#,
                r#"{"name": "fixture", "rootUri": "../", "packageUri": "lib/", "languageVersion": "3.5"}, "#,
                r#"{"name": "dep", "rootUri": "../../dq_cli_analyze_route_universe_dep", "packageUri": "lib/", "languageVersion": "3.5"}"#,
                r#"], "generator": "pub", "generatorVersion": "3.13.1"}"#,
            ),
        )
        .expect("the fresh config");

        let mut after: Vec<u8> = Vec::new();
        let code = route_analyze(
            &args,
            &targets,
            &mut after,
            &refused_spawn(),
            &mut RootMemo::default(),
        )
        .expect("the live session answers again");
        assert_eq!(
            code, 0,
            "the once-dangling import now resolves (no stale universe)"
        );
        assert!(
            String::from_utf8_lossy(&after).contains("diags:[]"),
            "the changed universe answers clean: {}",
            String::from_utf8_lossy(&after)
        );
        teardown(&root);
        let _ = std::fs::remove_dir_all(&dep);
    }

    /// The belt-and-braces gate (#2675): the routed answer's `packages`
    /// stamp decides print-vs-refuse against the disk — equal bytes are
    /// current, a moved mtime or size refuses, a config that appeared or
    /// vanished refuses, a `null` universe is current exactly while the
    /// conventional path holds no config, and an absent member carries no
    /// signal at all.
    #[test]
    fn the_universe_stamp_gate_decides_against_the_disk() {
        let root =
            crate::tests::analyze_fixture("route-stamp", &[("lib/main.dart", "int main() {}\n")]);
        let config = root.join(".dart_tool/package_config.json");
        let meta = std::fs::metadata(&config).expect("the fixture config stats");
        let mtime_ns = meta
            .modified()
            .expect("mtime")
            .duration_since(std::time::UNIX_EPOCH)
            .expect("post-epoch")
            .as_nanos() as u64;
        let path = config.display().to_string();
        let stamped =
            |mtime_ns: u64, size: u64| json!({"path": path, "mtime_ns": mtime_ns, "size": size});
        assert!(
            universe_matches_disk(&root, Some(&stamped(mtime_ns, meta.len()))),
            "the exact stamp is current"
        );
        assert!(
            !universe_matches_disk(&root, Some(&stamped(mtime_ns + 1, meta.len()))),
            "a moved mtime refuses"
        );
        assert!(
            !universe_matches_disk(&root, Some(&stamped(mtime_ns, meta.len() + 1))),
            "a moved size refuses"
        );
        assert!(
            !universe_matches_disk(&root, Some(&Value::Null)),
            "a null universe refuses while the disk holds a config"
        );
        assert!(
            universe_matches_disk(&root, None),
            "an absent member is no signal"
        );
        assert!(
            !universe_matches_disk(
                &root,
                Some(&json!({"path": "/nonexistent/config.json", "mtime_ns": 1, "size": 1})),
            ),
            "a config the daemon read that is gone now refuses"
        );
        std::fs::remove_file(&config).expect("the config leaves");
        assert!(
            universe_matches_disk(&root, Some(&Value::Null)),
            "null over no config is current"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// The check face consults an attached session (never spawning) and
    /// answers the cold pipeline's own bytes on every format; with
    /// nothing live it falls straight through — a syntax sweep never
    /// spawns an engine.
    #[test]
    fn check_consults_an_attached_session_with_the_cold_bytes() {
        let _serial = SIGNAL_SERIALIZATION
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        // A parse-broken library: the M0 pipeline's own rows keep the
        // byte identity honest (a tree that parses clean would pin only
        // the empty shape).
        let root =
            crate::tests::analyze_fixture("route-check", &[("lib/broken.dart", "void f( {\n")]);
        serve(&root);
        let path = root.display().to_string();
        for word in ["lon", "json", "text"] {
            let mut cold_args =
                crate::tests::check_args(&["check", "--format", word, path.as_str()]);
            cold_args.cold = true;
            let mut cold: Vec<u8> = Vec::new();
            let cold_code = crate::check_run(&cold_args, &mut cold);

            let mut routed: Vec<u8> = Vec::new();
            let code = route_check(
                &crate::tests::check_args(&["check", "--format", word, path.as_str()]),
                &mut routed,
            )
            .expect("the live session answers");
            assert_eq!(code, cold_code, "the exit tier matches ({word})");
            assert_eq!(
                String::from_utf8_lossy(&routed),
                String::from_utf8_lossy(&cold),
                "routed check stdout == cold stdout, byte for byte ({word})"
            );
        }

        let bare = broken_fixture("route-check-bare");
        let bare_path = bare.display().to_string();
        assert!(
            route_check(
                &crate::tests::check_args(&["check", bare_path.as_str()]),
                &mut Vec::new()
            )
            .is_none(),
            "no live session routes never"
        );
        teardown(&root);
        std::fs::remove_dir_all(&bare).ok();
    }

    /// The performance pin (#2627): a warm routed call — the session
    /// already holds the analysis — answers within the ticket's 1.5 s
    /// bound over a many-file fixture. The wall's anatomy (measured,
    /// debug): the serve loop's 250 ms accept tick, the daemon's drain
    /// (the incremental pass, small on an unchanged tree), and the
    /// election's triple compute — one content hash of the running
    /// binary, ~1 s on an 83 MB debug build and ~tens of ms in release
    /// (memoized per process; the in-process serve pays it at bring-up,
    /// so the warm calls here ride the memo). A comparative
    /// warm-vs-cold bound cannot be pinned in a unit test: the cold
    /// face's wall over a small fixture is a few engine assemblies
    /// (~80 ms measured), structurally under the resident's IPC floor —
    /// the resident wins from real tree sizes up (recon #2620: cold
    /// 3.9–5.0 s on a Flutter package), which the batch phase's release
    /// benchmark owns.
    #[test]
    fn a_warm_routed_call_answers_within_the_bound() {
        let _serial = SIGNAL_SERIALIZATION
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let mut libraries: Vec<(String, String)> = (0..80)
            .map(|i| {
                (
                    format!("lib/src/g_{i:03}.dart"),
                    format!("int g_{i:03}() => {i};\n"),
                )
            })
            .collect();
        libraries.push((
            String::from("lib/broken.dart"),
            String::from("void f() { x }\n"),
        ));
        let refs: Vec<(&str, &str)> = libraries
            .iter()
            .map(|(path, body)| (path.as_str(), body.as_str()))
            .collect();
        let root = crate::tests::analyze_fixture("route-warm", &refs);
        serve(&root);
        let targets = [TargetKind::Dir(root.clone())];
        let args = routed_args(&root, AnalyzeFormat::Lon);

        // Warm the resident: the first routed call pays the daemon's own
        // cold analysis inside the session (and populates this process's
        // binary-hash memo).
        route_analyze(
            &args,
            &targets,
            &mut Vec::new(),
            &refused_spawn(),
            &mut RootMemo::default(),
        )
        .expect("the first routed call answers");

        let warm_at = Instant::now();
        let mut warm: Vec<u8> = Vec::new();
        route_analyze(
            &args,
            &targets,
            &mut warm,
            &refused_spawn(),
            &mut RootMemo::default(),
        )
        .expect("the warm call answers");
        let warm_ms = warm_at.elapsed().as_millis();

        let mut cold: Vec<u8> = Vec::new();
        crate::analyze_run(
            &crate::tests::analyze_args(&root, AnalyzeFormat::Lon, None),
            &mut cold,
        );
        assert_eq!(
            String::from_utf8_lossy(&warm),
            String::from_utf8_lossy(&cold),
            "warm and cold agree on the bytes"
        );
        assert!(
            warm_ms <= 1500,
            "the warm routed call answers within the 1.5 s bound ({warm_ms} ms)"
        );
        teardown(&root);
    }

    /// Kill-the-daemon mid-work (#2627's ladder): the next routed call
    /// finds the killed session's residue — a fresh state naming a pid
    /// that died, a socket nothing answers — and the CLI still answers:
    /// the analyze face's route gives the fallback `None` (the refused
    /// spawn stands in for a bring-up that cannot happen), the check
    /// face runs the full composition (consult declined, in-process
    /// pipeline) over the same bytes.
    #[test]
    fn a_killed_daemon_falls_back_to_the_cold_answer() {
        let _serial = SIGNAL_SERIALIZATION
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let root = broken_fixture("route-kill");
        // The killed-mid-work residue: a just-exited child's pid under a
        // fresh heartbeat and the matching generation, and a plain file
        // where the socket was (no serve loop — nothing to race, and the
        // in-process serve's own terminate self-guard means a live
        // thread here could not model the kill).
        let layout = Layout::new(&root);
        std::fs::create_dir_all(&layout.dir).expect("the state dir");
        std::fs::create_dir_all(layout.socket.parent().expect("socket dir")).expect("dir");
        let mut child = std::process::Command::new("sleep")
            .arg("0")
            .spawn()
            .expect("the short-lived child");
        let residue_pid = child.id();
        let _ = child.wait();
        assert!(
            !dartq_session::pid_alive(residue_pid),
            "the modeled pid is dead"
        );
        SessionState {
            pid: residue_pid,
            heartbeat_ms: SessionState::now_ms(),
            triple: SessionTriple::compute(&root, &root.join("sdk")),
            debug_sessions: 0,
            used_ms: 0,
        }
        .write(&layout)
        .expect("the residue writes");
        std::fs::write(&layout.socket, b"").expect("the dead socket file");

        // The analyze face: the route falls back (never an error, never
        // a hang), and the cold answer stands behind it.
        let mut cold: Vec<u8> = Vec::new();
        let cold_code = crate::analyze_run(
            &crate::tests::analyze_args(&root, AnalyzeFormat::Lon, None),
            &mut cold,
        );
        let outcome = route_analyze(
            &routed_args(&root, AnalyzeFormat::Lon),
            &[TargetKind::Dir(root.clone())],
            &mut Vec::new(),
            &refused_spawn(),
            &mut RootMemo::default(),
        );
        assert!(outcome.is_none(), "the killed session falls back cold");

        // The check face's full composition: the consult is declined
        // (nothing live), the in-process pipeline answers the same bytes
        // the cold run printed.
        let path = root.display().to_string();
        let mut cold_check_args = crate::tests::check_args(&["check", path.as_str()]);
        cold_check_args.cold = true;
        let mut cold_check: Vec<u8> = Vec::new();
        crate::check_run(&cold_check_args, &mut cold_check);
        let mut answer: Vec<u8> = Vec::new();
        crate::check_run(
            &crate::tests::check_args(&["check", path.as_str()]),
            &mut answer,
        );
        assert_eq!(
            String::from_utf8_lossy(&answer),
            String::from_utf8_lossy(&cold_check),
            "the CLI still answers via the fallback"
        );
        assert!(
            !cold.is_empty(),
            "the broken fixture keeps the bytes honest"
        );
        assert_ne!(
            cold_code,
            crate::exit::CLEAN,
            "the dirty tree's exit tier rides the fallback unchanged"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A stale binary stamp (the recorded generation is not this
    /// binary's): the election reaps and takes over — bounded by its
    /// start window, never a hang — and the routed answer over the
    /// taken-over session is still the cold bytes.
    #[test]
    fn a_stale_binary_stamp_is_taken_over_not_hung_on() {
        let _serial = SIGNAL_SERIALIZATION
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let root = broken_fixture("route-stale");
        // The stale generation's state: a live pid (the harness's own —
        // the reaper's self-guard skips it), a fresh heartbeat, and a
        // foreign binary stamp. The triple's other fields stay true so
        // the takeover is the stamp's alone.
        let layout = Layout::new(&root);
        std::fs::create_dir_all(&layout.dir).expect("the state dir");
        let mut stale = SessionTriple::compute(&root, &root.join("sdk"));
        stale.binary_stamp = "an-older-generation".into();
        SessionState {
            pid: std::process::id(),
            heartbeat_ms: SessionState::now_ms(),
            triple: stale,
            debug_sessions: 0,
            used_ms: 0,
        }
        .write(&layout)
        .expect("the stale state writes");
        let spawn_root = root.clone();
        let spawn = move |_: &Layout, _: &SessionTriple| {
            let dir = spawn_root.clone();
            let sdk = spawn_root.join("sdk");
            std::thread::spawn(move || {
                crate::session::serve_loop_for_tests_with(&dir, &sdk, Duration::from_secs(30))
            });
            Ok(())
        };

        let mut cold: Vec<u8> = Vec::new();
        let cold_code = crate::analyze_run(
            &crate::tests::analyze_args(&root, AnalyzeFormat::Json, None),
            &mut cold,
        );
        let started = Instant::now();
        let mut routed: Vec<u8> = Vec::new();
        let code = route_analyze(
            &routed_args(&root, AnalyzeFormat::Json),
            &[TargetKind::Dir(root.clone())],
            &mut routed,
            &spawn,
            &mut RootMemo::default(),
        )
        .expect("the takeover answers");
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "the takeover is bounded by the election's start window, not a hang"
        );
        assert_eq!(code, cold_code, "the exit tier matches");
        assert_eq!(
            String::from_utf8_lossy(&routed),
            String::from_utf8_lossy(&cold),
            "the taken-over session answers the cold bytes"
        );
        teardown(&root);
    }

    /// `--cold` bypasses the routing (#2627): against a session state
    /// that looks live but is held by a sentinel listener, the default
    /// run consults (the sentinel counts the connection) while the
    /// `--cold` run never connects — the consult count is the one
    /// observable that separates them, the bytes being identical by
    /// design.
    #[test]
    fn cold_bypasses_the_consult() {
        use std::os::unix::net::UnixListener;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

        let _serial = SIGNAL_SERIALIZATION
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let root = broken_fixture("route-coldflag");
        // The live-looking session over a sentinel socket: same root,
        // same binary stamp, live pid, fresh heartbeat — every consult
        // connects to the sentinel, whose acceptor closes each stream
        // unread at once (the transport error's fast path; nothing
        // serves, nothing blocks on the call bound).
        let layout = Layout::new(&root);
        std::fs::create_dir_all(&layout.dir).expect("the state dir");
        std::fs::create_dir_all(layout.socket.parent().expect("socket dir")).expect("dir");
        let _ = std::fs::remove_file(&layout.socket);
        let sentinel =
            Arc::new(UnixListener::bind(&layout.socket).expect("the sentinel socket binds"));
        sentinel.set_nonblocking(true).expect("the sentinel polls");
        let hits = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        {
            let listener = Arc::clone(&sentinel);
            let hits = Arc::clone(&hits);
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            drop(stream);
                            hits.fetch_add(1, Ordering::Relaxed);
                        }
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        Err(_) => break,
                    }
                }
            });
        }
        let live_state = || SessionState {
            pid: std::process::id(),
            heartbeat_ms: SessionState::now_ms(),
            triple: SessionTriple::compute(&root, &root.join("sdk")),
            debug_sessions: 0,
            used_ms: 0,
        };
        live_state().write(&layout).expect("the state writes");

        // The default consults: the sentinel sees the round trips (the
        // ladder's attempts, probes included) and the refusals fall the
        // run back cold — the consult is the fact under test.
        let outcome = route_analyze(
            &routed_args(&root, AnalyzeFormat::Json),
            &[TargetKind::Dir(root.clone())],
            &mut Vec::new(),
            &refused_spawn(),
            &mut RootMemo::default(),
        );
        assert!(outcome.is_none(), "the sentinel's refusal falls back cold");
        std::thread::sleep(Duration::from_millis(100));
        let consulted = hits.load(Ordering::Relaxed);
        assert!(
            consulted >= 1,
            "the default run consults the session ({consulted} hits)"
        );

        // The `--cold` run never consults: zero new connections over
        // the same live-looking state, the same bytes (`analyze_args`
        // carries `cold: true` — the flag under test).
        live_state().write(&layout).expect("the state stays fresh");
        let cold_args = crate::tests::analyze_args(&root, AnalyzeFormat::Json, None);
        let mut cold: Vec<u8> = Vec::new();
        let cold_code = crate::analyze_run(&cold_args, &mut cold);
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(
            hits.load(Ordering::Relaxed),
            consulted,
            "--cold never consults the session"
        );
        assert!(
            !cold.is_empty(),
            "the broken fixture keeps the bytes honest"
        );
        assert_ne!(
            cold_code,
            crate::exit::CLEAN,
            "the dirty tree answers dirty"
        );

        stop.store(true, Ordering::Relaxed);
        drop(sentinel);
        let _ = std::fs::remove_file(&layout.socket);
        let _ = std::fs::remove_file(&layout.state);
        let canonical = root.canonicalize().unwrap_or_else(|_| root.clone());
        dartq_session::Registry::open().remove(&canonical);
        std::fs::remove_dir_all(&root).ok();
    }

    // -----------------------------------------------------------------------
    // The answer faces (#2629): query (every kind) + batch, explain,
    // impact, rename (dry run), resolve, complete
    // -----------------------------------------------------------------------

    /// The answer faces' fixture: a declaring library and its importing
    /// use site — every face has a real symbol to answer over.
    fn answer_fixture(tag: &str) -> PathBuf {
        crate::tests::analyze_fixture(
            tag,
            &[
                (
                    "lib/a.dart",
                    "class Foo {\n  void meth() {}\n}\nvoid foo() {}\n",
                ),
                (
                    "lib/b.dart",
                    "import 'a.dart';\nvoid main() {\n  Foo? f = Foo();\n  foo();\n}\n",
                ),
            ],
        )
    }

    fn symbol_args(root: &Path, name: &str, format: crate::OutputFormat) -> crate::SymbolArgs {
        crate::SymbolArgs {
            name: name.to_owned(),
            dir: root.to_path_buf(),
            format,
            verbosity: crate::Verbosity::Compact,
            sdk: Some(root.join("sdk")),
            fuzzy: false,
        }
    }

    fn batch_args(root: &Path, format: crate::OutputFormat) -> crate::BatchArgs {
        crate::BatchArgs {
            symbols: vec!["foo".into(), "Foo".into()],
            kinds: Vec::new(),
            verbosity: crate::Verbosity::Compact,
            format,
            dir: root.to_path_buf(),
            sdk: Some(root.join("sdk")),
        }
    }

    fn explain_args(root: &Path, format: crate::OutputFormat) -> crate::ExplainArgs {
        crate::ExplainArgs {
            symbol: Some("Foo".into()),
            diagnostic: None,
            file: None,
            dir: root.to_path_buf(),
            format,
            sdk: Some(root.join("sdk")),
            verbosity: crate::Verbosity::Compact,
        }
    }

    fn impact_args(
        root: &Path,
        format: crate::OutputFormat,
        symbols: Vec<String>,
    ) -> crate::impact::ImpactArgs {
        crate::impact::ImpactArgs {
            path: root.join("lib/a.dart"),
            dir: root.to_path_buf(),
            sdk: Some(root.join("sdk")),
            verbosity: crate::Verbosity::Compact,
            symbols,
            format,
        }
    }

    fn rename_args(root: &Path, format: crate::OutputFormat) -> crate::rename::RenameArgs {
        let a = root
            .canonicalize()
            .unwrap_or_else(|_| root.to_path_buf())
            .join("lib/a.dart");
        crate::rename::RenameArgs {
            target: format!("{}:4:6", a.display()),
            new_name: "gb".into(),
            write: false,
            format,
            dir: root.to_path_buf(),
            sdk: Some(root.join("sdk")),
        }
    }

    fn position_args(
        root: &Path,
        selector: &str,
        format: crate::OutputFormat,
    ) -> crate::resolve::PositionArgs {
        crate::resolve::PositionArgs {
            selector: selector.to_owned(),
            dir: root.to_path_buf(),
            format,
            sdk: Some(root.join("sdk")),
        }
    }

    /// The money pin per face (#2629): the routed answer's stdout is the
    /// cold path's stdout, byte for byte, and the exit tier (0) matches —
    /// the daemon renders through the same writers the cold face prints
    /// with. The cold passes run first (nothing live — the attach-only
    /// consult declines silently), then the serve loop answers.
    #[test]
    fn routed_query_matches_the_cold_bytes_on_every_format() {
        let _serial = SIGNAL_SERIALIZATION
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let root = answer_fixture("route-q");
        let formats = [
            (crate::OutputFormat::Lon, "lon"),
            (crate::OutputFormat::Json, "json"),
            (crate::OutputFormat::Text, "text"),
        ];
        let mut cold: Vec<(crate::OutputFormat, Vec<u8>)> = Vec::new();
        for (format, _) in formats {
            let mut bytes: Vec<u8> = Vec::new();
            let code = crate::query_symbol_run(&symbol_args(&root, "foo", format), &mut bytes);
            assert_eq!(code, exit::CLEAN);
            assert!(!bytes.is_empty(), "the fixture keeps the bytes honest");
            cold.push((format, bytes));
        }
        serve(&root);
        for (format, cold) in cold {
            let mut routed: Vec<u8> = Vec::new();
            let code = crate::query_symbol_run(&symbol_args(&root, "foo", format), &mut routed);
            assert_eq!(code, exit::CLEAN);
            assert_eq!(
                String::from_utf8_lossy(&routed),
                String::from_utf8_lossy(&cold),
                "routed query stdout == cold stdout, byte for byte"
            );
        }
        teardown(&root);
    }

    /// The batch face rides the same identity (#2629): one round trip over
    /// the session graph, the same per-line envelopes, byte for byte.
    #[test]
    fn routed_batch_matches_the_cold_bytes() {
        let _serial = SIGNAL_SERIALIZATION
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let root = answer_fixture("route-qb");
        let mut cold: Vec<u8> = Vec::new();
        let code = crate::query_batch_run(&batch_args(&root, crate::OutputFormat::Lon), &mut cold);
        assert_eq!(code, exit::CLEAN);
        assert!(cold.len() > 1, "two items answer two lines");
        serve(&root);
        let mut routed: Vec<u8> = Vec::new();
        let code =
            crate::query_batch_run(&batch_args(&root, crate::OutputFormat::Lon), &mut routed);
        assert_eq!(code, exit::CLEAN);
        assert_eq!(
            String::from_utf8_lossy(&routed),
            String::from_utf8_lossy(&cold),
            "routed batch stdout == cold stdout, byte for byte"
        );
        teardown(&root);
    }

    /// The explain face (#2629): attach-only — the daemon rebuilds the
    /// same per-query disk engine and renders the same report.
    #[test]
    fn routed_explain_matches_the_cold_bytes() {
        let _serial = SIGNAL_SERIALIZATION
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let root = answer_fixture("route-ex");
        let mut cold: Vec<u8> = Vec::new();
        let code = crate::explain_run(&explain_args(&root, crate::OutputFormat::Lon), &mut cold);
        assert_eq!(code, exit::CLEAN);
        assert!(!cold.is_empty(), "the report prints");
        serve(&root);
        let mut routed: Vec<u8> = Vec::new();
        let code = crate::explain_run(&explain_args(&root, crate::OutputFormat::Lon), &mut routed);
        assert_eq!(code, exit::CLEAN);
        assert_eq!(
            String::from_utf8_lossy(&routed),
            String::from_utf8_lossy(&cold),
            "routed explain stdout == cold stdout, byte for byte"
        );
        teardown(&root);
    }

    /// The impact face (#2629), file-level and `--symbol`-narrowed (#2666):
    /// the routed report is the cold report, byte for byte, both postures.
    #[test]
    fn routed_impact_matches_the_cold_bytes() {
        let _serial = SIGNAL_SERIALIZATION
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let root = answer_fixture("route-im");
        let postures: [(Vec<String>, &str); 2] = [
            (Vec::new(), "file-level"),
            (vec!["foo".to_owned()], "narrowed"),
        ];
        let mut cold: Vec<(Vec<String>, Vec<u8>)> = Vec::new();
        for (symbols, _) in &postures {
            let mut bytes: Vec<u8> = Vec::new();
            let code = crate::impact::run(
                &impact_args(&root, crate::OutputFormat::Lon, symbols.clone()),
                &mut bytes,
            );
            assert_eq!(code, exit::CLEAN, "the cold face answers ({symbols:?})");
            cold.push((symbols.clone(), bytes));
        }
        serve(&root);
        for (symbols, cold) in cold {
            let mut routed: Vec<u8> = Vec::new();
            let code = crate::impact::run(
                &impact_args(&root, crate::OutputFormat::Lon, symbols.clone()),
                &mut routed,
            );
            assert_eq!(code, exit::CLEAN);
            assert_eq!(
                String::from_utf8_lossy(&routed),
                String::from_utf8_lossy(&cold),
                "routed impact stdout == cold stdout, byte for byte ({symbols:?})"
            );
        }
        teardown(&root);
    }

    /// The rename dry run (#2629): the daemon answers the edit-set
    /// document, the same bytes the cold plan prints.
    #[test]
    fn routed_rename_matches_the_cold_bytes() {
        let _serial = SIGNAL_SERIALIZATION
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let root = answer_fixture("route-rn");
        let mut cold: Vec<u8> = Vec::new();
        let code = crate::rename::run(&rename_args(&root, crate::OutputFormat::Lon), &mut cold);
        assert_eq!(code, exit::CLEAN);
        assert!(
            String::from_utf8_lossy(&cold).contains("new_name:gb"),
            "{cold:?}"
        );
        serve(&root);
        let mut routed: Vec<u8> = Vec::new();
        let code = crate::rename::run(&rename_args(&root, crate::OutputFormat::Lon), &mut routed);
        assert_eq!(code, exit::CLEAN);
        assert_eq!(
            String::from_utf8_lossy(&routed),
            String::from_utf8_lossy(&cold),
            "routed rename stdout == cold stdout, byte for byte"
        );
        teardown(&root);
    }

    /// The position faces (#2629): `resolve` and `complete` answer the
    /// same envelopes over the session's maintained view; the line/col
    /// cursor converts against the analyzed text exactly where the cold
    /// face converts against its own.
    #[test]
    fn routed_resolve_and_complete_match_the_cold_bytes() {
        let _serial = SIGNAL_SERIALIZATION
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let root = answer_fixture("route-pos");
        let resolve = position_args(&root, "lib/b.dart:4:3", crate::OutputFormat::Lon);
        let complete = position_args(&root, "lib/b.dart:4:5", crate::OutputFormat::Lon);
        let mut cold_resolve: Vec<u8> = Vec::new();
        let mut cold_complete: Vec<u8> = Vec::new();
        assert_eq!(
            crate::resolve::resolve_at_run(&resolve, &mut cold_resolve),
            exit::CLEAN
        );
        assert_eq!(
            crate::resolve::completion_run(&complete, &mut cold_complete),
            exit::CLEAN
        );
        assert!(!cold_resolve.is_empty() && !cold_complete.is_empty());
        serve(&root);
        let mut routed: Vec<u8> = Vec::new();
        assert_eq!(
            crate::resolve::resolve_at_run(&resolve, &mut routed),
            exit::CLEAN
        );
        assert_eq!(
            String::from_utf8_lossy(&routed),
            String::from_utf8_lossy(&cold_resolve),
            "routed resolve stdout == cold stdout"
        );
        let mut routed: Vec<u8> = Vec::new();
        assert_eq!(
            crate::resolve::completion_run(&complete, &mut routed),
            exit::CLEAN
        );
        assert_eq!(
            String::from_utf8_lossy(&routed),
            String::from_utf8_lossy(&cold_complete),
            "routed complete stdout == cold stdout"
        );
        teardown(&root);
    }

    /// The default consults (#2629's "by default"): against a live-looking
    /// session held by a sentinel listener, every answer face's run
    /// consults (the sentinel counts the connections) and still answers —
    /// the sentinel's refusals fall the runs back cold.
    #[test]
    fn the_answer_faces_consult_the_session_by_default() {
        use std::os::unix::net::UnixListener;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let _serial = SIGNAL_SERIALIZATION
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let root = answer_fixture("route-consult");
        let layout = Layout::new(&root);
        std::fs::create_dir_all(&layout.dir).expect("the state dir");
        std::fs::create_dir_all(layout.socket.parent().expect("socket dir")).expect("dir");
        let _ = std::fs::remove_file(&layout.socket);
        let sentinel =
            Arc::new(UnixListener::bind(&layout.socket).expect("the sentinel socket binds"));
        sentinel.set_nonblocking(true).expect("the sentinel polls");
        let hits = Arc::new(AtomicUsize::new(0));
        {
            let listener = Arc::clone(&sentinel);
            let hits = Arc::clone(&hits);
            std::thread::spawn(move || {
                loop {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            drop(stream);
                            hits.fetch_add(1, Ordering::Relaxed);
                        }
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        Err(_) => break,
                    }
                }
            });
        }
        let live_state = || SessionState {
            pid: std::process::id(),
            heartbeat_ms: SessionState::now_ms(),
            triple: SessionTriple::compute(&root, &root.join("sdk")),
            debug_sessions: 0,
            used_ms: 0,
        };
        live_state().write(&layout).expect("the state writes");

        let consulted = |face: &str| {
            live_state().write(&layout).expect("the state stays fresh");
            let before = hits.load(Ordering::Relaxed);
            let face_call: Box<dyn FnOnce() -> u8> = match face {
                "query" => Box::new(|| {
                    let mut out = Vec::new();
                    crate::query_symbol_run(
                        &symbol_args(&root, "foo", crate::OutputFormat::Lon),
                        &mut out,
                    );
                    exit::CLEAN
                }),
                "batch" => Box::new(|| {
                    let mut out = Vec::new();
                    crate::query_batch_run(&batch_args(&root, crate::OutputFormat::Lon), &mut out);
                    exit::CLEAN
                }),
                "explain" => Box::new(|| {
                    let mut out = Vec::new();
                    crate::explain_run(&explain_args(&root, crate::OutputFormat::Lon), &mut out);
                    exit::CLEAN
                }),
                "impact" => Box::new(|| {
                    let mut out = Vec::new();
                    crate::impact::run(
                        &impact_args(&root, crate::OutputFormat::Lon, Vec::new()),
                        &mut out,
                    );
                    exit::CLEAN
                }),
                "rename" => Box::new(|| {
                    let mut out = Vec::new();
                    crate::rename::run(&rename_args(&root, crate::OutputFormat::Lon), &mut out);
                    exit::CLEAN
                }),
                "resolve" => Box::new(|| {
                    let mut out = Vec::new();
                    crate::resolve::resolve_at_run(
                        &position_args(&root, "lib/b.dart:4:3", crate::OutputFormat::Lon),
                        &mut out,
                    );
                    exit::CLEAN
                }),
                "complete" => Box::new(|| {
                    let mut out = Vec::new();
                    crate::resolve::completion_run(
                        &position_args(&root, "lib/b.dart:4:5", crate::OutputFormat::Lon),
                        &mut out,
                    );
                    exit::CLEAN
                }),
                _ => unreachable!("the face table"),
            };
            face_call();
            std::thread::sleep(Duration::from_millis(100));
            let gained = hits.load(Ordering::Relaxed) - before;
            assert!(gained >= 1, "{face} consults the session ({gained} hits)");
        };
        for face in [
            "query", "batch", "explain", "impact", "rename", "resolve", "complete",
        ] {
            consulted(face);
        }

        drop(sentinel);
        let _ = std::fs::remove_file(&layout.socket);
        let _ = std::fs::remove_file(&layout.state);
        let canonical = root.canonicalize().unwrap_or_else(|_| root.clone());
        dartq_session::Registry::open().remove(&canonical);
        std::fs::remove_dir_all(&root).ok();
    }

    /// Kill-the-daemon mid-work (#2629's fallback): the next call finds the
    /// killed session's residue — a fresh state naming a pid that died, a
    /// socket nothing answers — and every answer face still answers with
    /// the cold bytes.
    #[test]
    fn a_killed_daemon_falls_back_cold_on_the_answer_faces() {
        let _serial = SIGNAL_SERIALIZATION
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let root = answer_fixture("route-kill2");
        // The cold bytes first, over a quiet tree.
        let mut cold_query: Vec<u8> = Vec::new();
        crate::query_symbol_run(
            &symbol_args(&root, "foo", crate::OutputFormat::Lon),
            &mut cold_query,
        );
        let mut cold_impact: Vec<u8> = Vec::new();
        crate::impact::run(
            &impact_args(&root, crate::OutputFormat::Lon, Vec::new()),
            &mut cold_impact,
        );
        let mut cold_rename: Vec<u8> = Vec::new();
        crate::rename::run(
            &rename_args(&root, crate::OutputFormat::Lon),
            &mut cold_rename,
        );

        // The killed-mid-work residue (the analyze test's shape): a dead
        // child's pid under a fresh heartbeat, a plain file where the
        // socket was.
        let layout = Layout::new(&root);
        std::fs::create_dir_all(&layout.dir).expect("the state dir");
        std::fs::create_dir_all(layout.socket.parent().expect("socket dir")).expect("dir");
        let mut child = std::process::Command::new("sleep")
            .arg("0")
            .spawn()
            .expect("child");
        let residue_pid = child.id();
        let _ = child.wait();
        SessionState {
            pid: residue_pid,
            heartbeat_ms: SessionState::now_ms(),
            triple: SessionTriple::compute(&root, &root.join("sdk")),
            debug_sessions: 0,
            used_ms: 0,
        }
        .write(&layout)
        .expect("the residue writes");
        std::fs::write(&layout.socket, b"").expect("the dead socket file");

        let mut query: Vec<u8> = Vec::new();
        crate::query_symbol_run(
            &symbol_args(&root, "foo", crate::OutputFormat::Lon),
            &mut query,
        );
        assert_eq!(
            String::from_utf8_lossy(&query),
            String::from_utf8_lossy(&cold_query),
            "the query face still answers via the fallback"
        );
        let mut impact: Vec<u8> = Vec::new();
        crate::impact::run(
            &impact_args(&root, crate::OutputFormat::Lon, Vec::new()),
            &mut impact,
        );
        assert_eq!(
            String::from_utf8_lossy(&impact),
            String::from_utf8_lossy(&cold_impact),
            "the impact face still answers via the fallback"
        );
        let mut rename: Vec<u8> = Vec::new();
        crate::rename::run(&rename_args(&root, crate::OutputFormat::Lon), &mut rename);
        assert_eq!(
            String::from_utf8_lossy(&rename),
            String::from_utf8_lossy(&cold_rename),
            "the rename face still answers via the fallback"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Freshness for the engine-view family (#2629's hard requirement,
    /// pinned on the query face — impact/rename/resolve/complete ride the
    /// same drain): an out-of-band edit (no sync notification) lands in
    /// the very next routed call.
    #[test]
    fn a_routed_query_reconciles_out_of_band_edits() {
        let _serial = SIGNAL_SERIALIZATION
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let root = answer_fixture("route-fresh2");
        serve(&root);
        let args = symbol_args(&root, "bar", crate::OutputFormat::Lon);
        let mut before: Vec<u8> = Vec::new();
        let code = crate::query_symbol_run(&args, &mut before);
        assert_eq!(code, exit::CLEAN);
        assert!(
            !String::from_utf8_lossy(&before).contains("name:bar"),
            "the fixture starts without bar"
        );

        // The edit adds the declaration; no notification rides — the next
        // plain request must reconcile against the disk.
        std::fs::write(
            root.join("lib/a.dart"),
            "class Foo {\n  void meth() {}\n}\nvoid foo() {}\nvoid bar() {}\n",
        )
        .expect("edit");
        let mut after: Vec<u8> = Vec::new();
        let code = crate::query_symbol_run(&args, &mut after);
        assert_eq!(code, exit::CLEAN);
        assert!(
            String::from_utf8_lossy(&after).contains("name:bar"),
            "the edit lands in the next routed answer: {}",
            String::from_utf8_lossy(&after)
        );
        teardown(&root);
    }

    /// The frame reader's announcement cap (#4276): an over-cap
    /// Content-Length is the structured Malformed arm — nothing is
    /// allocated and nothing is read, matching the analyzer-host face
    /// path's over-cap refusal. The body is never sent: a reader that
    /// honored the announcement would block on it and surface as
    /// `Wedged` under the deadline, so the Malformed verdict doubles as
    /// the fail-closed proof.
    #[test]
    fn read_frame_body_refuses_an_announcement_over_the_cap() {
        let (mut mine, mut theirs) = UnixStream::pair().expect("socketpair");
        let announced = MAX_FRAME_BODY_BYTES + 1;
        write!(mine, "Content-Length: {announced}\r\n\r\n").expect("write header");
        mine.flush().expect("flush");
        theirs
            .set_read_timeout(Some(Duration::from_millis(50)))
            .expect("timeout");
        let answer = read_frame_body(&mut theirs, &mut Vec::new());
        let expected = format!(
            "frame announces {announced} bytes, over the {MAX_FRAME_BODY_BYTES}-byte cap"
        );
        assert!(
            matches!(
                &answer,
                Err(ReadError::Malformed(message)) if *message == expected
            ),
            "an over-cap announcement fails closed: {answer:?}"
        );
    }

    /// The cap's boundary (#4276): the announcement check is strictly
    /// greater, so a body exactly at the cap and one just under it read
    /// whole — the cap refuses only what it must. The cap rides as a
    /// parameter to exercise the boundary without a gigabyte fixture.
    #[test]
    fn read_frame_body_reads_frames_at_and_under_the_cap() {
        let cap = 64;
        let (mut mine, mut theirs) = UnixStream::pair().expect("socketpair");
        let under = vec![b'u'; cap - 1];
        let at = vec![b'a'; cap];
        // One write, both frames: the second also exercises the carried
        // overlap beside the boundary.
        for body in [&under, &at] {
            write!(mine, "Content-Length: {}\r\n\r\n", body.len()).expect("write header");
            mine.write_all(body).expect("write body");
        }
        mine.flush().expect("flush");
        let mut leftover = Vec::new();
        let first =
            read_frame_body_capped(&mut theirs, &mut leftover, cap).expect("a body under the cap");
        assert_eq!(first, under, "one byte under the cap reads whole");
        let second =
            read_frame_body_capped(&mut theirs, &mut leftover, cap).expect("a body at the cap");
        assert_eq!(second, at, "a body at the cap reads whole");
    }
}
