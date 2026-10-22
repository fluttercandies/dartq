//! The synchronous VM Service connection: one dedicated connection
//! thread owns the single blocking WebSocket — request texts arrive on
//! one channel, get written to the socket, and each inbound frame is
//! dispatched as a JSON-RPC response (matched by id to a pending
//! one-shot waiter) or a `streamNotify` event (pushed to a bounded
//! channel). A stalled consumer never blocks the connection thread.
//!
//! Protocol source: "Dart VM Service Protocol"
//! (`runtime/vm/service/service.md` in the upstream Dart SDK) — the
//! request/response/event envelope is JSON-RPC 2.0 with named params.

use std::net::TcpStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use crossbeam_channel::{Receiver, SendTimeoutError, Sender, bounded};
use parking_lot::Mutex;
use rustc_hash::FxHashMap;
use serde::Deserialize as _;
use serde_json::{Value, json};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

use crate::error::{Error, Result};
use crate::protocol::Event;

/// Default timeout for a single RPC round-trip. VM operations that
/// can take longer (reload of a large app) lift it per call.
const DEFAULT_RPC_TIMEOUT: Duration = Duration::from_secs(30);

/// How often the connection thread wakes to drain the write channel
/// and check for shutdown when no inbound frame arrives.
///
/// This tick *is* the RPC latency floor: a request queued while the
/// thread is blocked in `socket.read()` waits out the remainder of the
/// tick before it hits the wire. At 20 ms the `getStack` inside a
/// stopped snapshot cost ~20 ms of pure queue time (measured: queued
/// wake p95 ≈ 24 ms). At 1 ms an idle loop costs one `try_recv` plus
/// one `flush` syscall per millisecond — negligible CPU — while a
/// queued request waits at most one tick. The alternative (waking the
/// reader on write) needs a self-pipe `select!`ed against the socket
/// fd, which tungstenite's blocking API cannot express without
/// unsafe/polling tricks — not worth it for a sync blocking socket.
const READ_TICK: Duration = Duration::from_millis(1);

/// Upper bound on one blocking socket write. A peer that stops reading
/// for this long is treated as dead: the connection fails and every
/// pending call is answered [`Error::Closed`], which also bounds how
/// long the last handle's drop can wait on the connection thread.
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);

/// How many stream events the connection buffers. Consumers drain via
/// [`VmClient::drain_events`]/[`VmClient::next_event`]; overruns drop
/// the newest entries so a stalled consumer cannot grow memory or
/// block the reader — [`VmClient::dropped_events`] counts them.
pub const EVENT_BUFFER_CAPACITY: usize = 10_000;

/// How many outbound request texts may wait unwritten. Writes drain
/// every [`READ_TICK`]; a caller facing a full queue waits up to its
/// own RPC timeout for room rather than failing spuriously.
const WRITE_QUEUE_CAPACITY: usize = 1024;

/// One routed answer.
type Answer = std::result::Result<Value, Error>;

/// In-flight waiters by request id, plus the terminal flag. Both live
/// under one lock so a call can never register a waiter after the
/// connection has failed every pending one (that waiter would sit out
/// its full timeout instead of seeing [`Error::Closed`]).
#[derive(Default)]
struct Pending {
    waiters: FxHashMap<u64, Sender<Answer>>,
    closed: bool,
}

struct Shared {
    pending: Mutex<Pending>,
    next_id: AtomicU64,
    /// Sequence numbering for stream events, assigned on the
    /// connection thread in wire order.
    event_seq: AtomicU64,
    /// Every sequence number below this has been delivered to the feed
    /// or counted in `dropped_events` (see [`VmClient::settled_events`]).
    settled_events: AtomicU64,
    /// Events dropped because the bounded event feed was full —
    /// consumers surface the gap through [`VmClient::dropped_events`].
    dropped_events: AtomicU64,
    /// Inbound frames discarded as unparseable or shape-violating
    /// (not JSON, events without the required fields). Surfaced
    /// through [`VmClient::malformed_frames`] so a broken peer is
    /// visible rather than silent.
    malformed_frames: AtomicU64,
    /// Set by [`VmClient::close`] and the last handle's drop; the
    /// connection thread checks it every tick, so shutdown never
    /// depends on room in the write queue.
    shutdown: AtomicBool,
    /// Live [`VmClient`] handles; the last one to drop joins the
    /// connection thread.
    handles: AtomicUsize,
    /// The connection thread, joined once by the last dropping handle
    /// so a dropped client never leaves the thread/socket alive.
    conn_thread: Mutex<Option<JoinHandle<()>>>,
}

/// One decoded stream event plus its sequence number in the
/// connection's event feed.
#[derive(Debug, Clone)]
pub struct StreamEvent {
    /// Monotonic per-connection sequence number, assigned in wire
    /// order. Events dropped by the bounded feed still consume a
    /// number, so a jump between consecutive events is an exact gap.
    pub seq: u64,
    /// The `streamId` the VM tagged this event with (`Debug`,
    /// `Isolate`, `Stdout`, `Stderr`, `Logging`, …).
    pub stream_id: String,
    /// The parsed event payload.
    pub event: Event,
    /// The raw event JSON, for consumers that need fields outside the
    /// typed model.
    pub raw: Value,
}

/// A connected VM Service endpoint. Cheap to clone — clones share the
/// one socket, pending table, and event feed.
pub struct VmClient {
    shared: Arc<Shared>,
    events: Receiver<StreamEvent>,
    writes: Sender<String>,
}

impl VmClient {
    /// Connects a `ws://` or `wss://` endpoint and spawns the
    /// connection thread. `ws_uri` is the full WebSocket URI including
    /// the auth path segment (`ws://127.0.0.1:PORT/<auth>=/ws`).
    ///
    /// # Errors
    ///
    /// Fails on TCP connect, TLS, or WebSocket handshake errors, or
    /// when the socket cannot enter the read-tick timeout mode.
    pub fn connect(ws_uri: &str) -> Result<Self> {
        let (socket, _response) = tungstenite::client::connect_with_config(ws_uri, Some(ws_config()), 3)?;
        Self::from_socket(socket)
    }

    /// Builds a client over an already-established socket — the
    /// testable seam (unit tests hand in an accepted loopback socket).
    ///
    /// # Errors
    ///
    /// Fails when the socket cannot enter the read-tick timeout mode
    /// or the connection thread cannot spawn.
    pub fn from_socket(mut socket: WebSocket<MaybeTlsStream<TcpStream>>) -> Result<Self> {
        // The read timeout is the connection thread's heartbeat: on
        // each tick it drains the write channel. ws and wss share this
        // path — no stream splitting needed.
        set_timeouts(&mut socket)?;
        socket.set_config(|config| *config = ws_config());
        let (event_sink, events) = bounded(EVENT_BUFFER_CAPACITY);
        let (writes, write_rx) = bounded(WRITE_QUEUE_CAPACITY);
        let shared = Arc::new(Shared {
            pending: Mutex::new(Pending::default()),
            next_id: AtomicU64::new(1),
            event_seq: AtomicU64::new(0),
            settled_events: AtomicU64::new(0),
            dropped_events: AtomicU64::new(0),
            malformed_frames: AtomicU64::new(0),
            shutdown: AtomicBool::new(false),
            handles: AtomicUsize::new(1),
            conn_thread: Mutex::new(None),
        });
        let thread_shared = Arc::clone(&shared);
        let conn_thread = std::thread::Builder::new()
            .name("dartq-debug-conn".into())
            .spawn(move || connection_loop(&mut socket, &thread_shared, &write_rx, &event_sink))
            .map_err(Error::Io)?;
        *shared.conn_thread.lock() = Some(conn_thread);
        Ok(Self {
            shared,
            events,
            writes,
        })
    }

    /// Sends one RPC and waits for its response. `method` is the
    /// protocol method name (`getStack`, `resume`, …); `params` is the
    /// named-parameter object.
    ///
    /// # Errors
    ///
    /// [`Error::Rpc`] when the VM returns a JSON-RPC error object;
    /// [`Error::Timeout`] when the write queue stays full or the
    /// round-trip exceeds `timeout`; [`Error::Closed`] if the
    /// connection is down or drops mid-flight.
    pub fn call(&self, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        let id = self.shared.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = bounded(1);
        {
            let mut pending = self.shared.pending.lock();
            if pending.closed {
                return Err(Error::Closed);
            }
            pending.waiters.insert(id, tx);
        }
        let request = json!({
            "jsonrpc": "2.0",
            "id": id.to_string(),
            "method": method,
            "params": params,
        });
        // A burst can fill the write queue; wait for room within the
        // caller's own budget instead of failing a healthy connection.
        match self.writes.send_timeout(request.to_string(), timeout) {
            Ok(()) => {}
            Err(SendTimeoutError::Timeout(_)) => {
                self.forget(id);
                return Err(Error::Timeout(format!(
                    "rpc {method}: write queue full for {timeout:?}"
                )));
            }
            Err(SendTimeoutError::Disconnected(_)) => {
                self.forget(id);
                return Err(Error::Closed);
            }
        }
        match rx.recv_timeout(timeout) {
            Ok(answer) => answer,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                // Unregister so a peer that never answers cannot grow
                // the pending table; a late answer is then discarded
                // as an unknown id.
                self.forget(id);
                Err(Error::Timeout(format!("rpc {method} after {timeout:?}")))
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => Err(Error::Closed),
        }
    }

    fn forget(&self, id: u64) {
        self.shared.pending.lock().waiters.remove(&id);
    }

    /// Sends one RPC and deserializes `result` into `T`. A `Sentinel`
    /// result becomes [`Error::Sentinel`].
    ///
    /// # Errors
    ///
    /// [`Error::Protocol`] when the payload does not match `T`;
    /// otherwise see [`VmClient::call`].
    pub fn call_typed<T>(&self, method: &str, params: Value, timeout: Duration) -> Result<T>
    where
        T: serde::de::DeserializeOwned,
    {
        let value = self.call(method, params, timeout)?;
        check_not_sentinel(&value)?;
        serde_json::from_value(value)
            .map_err(|e| Error::Protocol(format!("decoding {method} result: {e}")))
    }

    /// Sends one RPC expecting the empty `Success` result; sentinels
    /// still fail as [`Error::Sentinel`].
    ///
    /// # Errors
    ///
    /// See [`VmClient::call`].
    pub fn call_unit(&self, method: &str, params: Value, timeout: Duration) -> Result<()> {
        let value = self.call(method, params, timeout)?;
        check_not_sentinel(&value)
    }

    /// Drains buffered events in arrival order. Non-blocking.
    pub fn drain_events(&self) -> Vec<StreamEvent> {
        self.events.try_iter().collect()
    }

    /// Waits for the next buffered event, up to `timeout`.
    ///
    /// # Errors
    ///
    /// [`Error::Closed`] when the feed disconnects (connection thread
    /// gone) and nothing is buffered; `Ok(None)` on timeout.
    pub fn next_event(&self, timeout: Duration) -> Result<Option<StreamEvent>> {
        match self.events.recv_timeout(timeout) {
            Ok(event) => Ok(Some(event)),
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => Ok(None),
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => Err(Error::Closed),
        }
    }

    /// The protocol `getVersion` RPC.
    ///
    /// # Errors
    ///
    /// See [`VmClient::call_typed`].
    pub fn get_version(&self) -> Result<crate::protocol::Version> {
        self.call_typed("getVersion", json!({}), DEFAULT_RPC_TIMEOUT)
    }

    /// The protocol `getVM` RPC.
    ///
    /// # Errors
    ///
    /// See [`VmClient::call_typed`].
    pub fn get_vm(&self) -> Result<crate::protocol::Vm> {
        self.call_typed("getVM", json!({}), DEFAULT_RPC_TIMEOUT)
    }

    /// `streamListen` for one stream id (`Debug`, `Isolate`, `Stdout`,
    /// `Stderr`, `Logging`, …).
    ///
    /// # Errors
    ///
    /// RPC error 103 when already subscribed; otherwise see
    /// [`VmClient::call`].
    pub fn stream_listen(&self, stream_id: &str) -> Result<()> {
        self.call_unit(
            "streamListen",
            json!({ "streamId": stream_id }),
            DEFAULT_RPC_TIMEOUT,
        )
    }

    /// `streamCancel` for one stream id.
    ///
    /// # Errors
    ///
    /// RPC error 104 when not subscribed; otherwise see
    /// [`VmClient::call`].
    pub fn stream_cancel(&self, stream_id: &str) -> Result<()> {
        self.call_unit(
            "streamCancel",
            json!({ "streamId": stream_id }),
            DEFAULT_RPC_TIMEOUT,
        )
    }

    /// Stream events dropped because the bounded feed was full — a
    /// consumer that fell behind sees the gap here rather than losing
    /// events silently.
    pub fn dropped_events(&self) -> u64 {
        self.shared.dropped_events.load(Ordering::Relaxed)
    }

    /// Sequence numbers the connection has finished with — every event
    /// below this mark was either delivered to the feed or counted as
    /// dropped. A consumer that drains the feed *after* reading the
    /// mark has seen every delivered event below it, so any remaining
    /// hole below the mark is an exact drop.
    pub fn settled_events(&self) -> u64 {
        self.shared.settled_events.load(Ordering::Acquire)
    }

    /// Inbound frames discarded as malformed (not JSON, or a
    /// `streamNotify` whose event violates the protocol shape).
    pub fn malformed_frames(&self) -> u64 {
        self.shared.malformed_frames.load(Ordering::Relaxed)
    }

    /// Closes the connection: pending and later RPCs answer
    /// [`Error::Closed`] and the connection thread exits within one
    /// tick. Never blocks, whatever the write-queue state.
    pub fn close(&self) {
        self.shared.shutdown.store(true, Ordering::Release);
    }
}

impl Clone for VmClient {
    fn clone(&self) -> Self {
        self.shared.handles.fetch_add(1, Ordering::Relaxed);
        Self {
            shared: Arc::clone(&self.shared),
            events: self.events.clone(),
            writes: self.writes.clone(),
        }
    }
}

impl Drop for VmClient {
    fn drop(&mut self) {
        // The last handle shuts the connection down and joins its
        // thread; the wait is bounded by one read tick plus at most
        // one blocked write (`WRITE_TIMEOUT`).
        if self.shared.handles.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.close();
            let thread = self.shared.conn_thread.lock().take();
            if let Some(thread) = thread {
                // A panicked connection thread has already failed
                // every waiter's channel; nothing more to report here.
                let _ = thread.join();
            }
        }
    }
}

/// The transport limits. The Dart VM writes each message as one
/// WebSocket frame, so the frame cap must equal the message cap —
/// tungstenite's 16 MiB default frame cap would otherwise kill the
/// connection on a large `getScripts`/`getObject` response long before
/// the 64 MiB message bound applies.
fn ws_config() -> tungstenite::protocol::WebSocketConfig {
    const MAX_MESSAGE_BYTES: usize = 64 << 20;
    tungstenite::protocol::WebSocketConfig::default()
        .max_message_size(Some(MAX_MESSAGE_BYTES))
        .max_frame_size(Some(MAX_MESSAGE_BYTES))
}

/// Applies the connection thread's read tick and write bound to the
/// transport underneath the WebSocket.
fn set_timeouts(socket: &mut WebSocket<MaybeTlsStream<TcpStream>>) -> Result<()> {
    let stream = match socket.get_mut() {
        MaybeTlsStream::Plain(stream) => stream,
        MaybeTlsStream::Rustls(stream) => &mut stream.sock,
        _ => {
            return Err(Error::Protocol(
                "unsupported WebSocket transport for read tick".to_owned(),
            ));
        }
    };
    stream.set_read_timeout(Some(READ_TICK))?;
    stream.set_write_timeout(Some(WRITE_TIMEOUT))?;
    Ok(())
}

fn connection_loop(
    socket: &mut WebSocket<MaybeTlsStream<TcpStream>>,
    shared: &Shared,
    writes: &Receiver<String>,
    sink: &Sender<StreamEvent>,
) {
    loop {
        if shared.shutdown.load(Ordering::Acquire) {
            // Best-effort close frame: the peer may already be gone.
            let _ = socket.close(None);
            let _ = socket.flush();
            break;
        }
        // Queue every pending request, then flush once: a burst costs
        // one syscall, and requests hit the wire even when the peer is
        // silent.
        let mut failed = false;
        while let Ok(text) = writes.try_recv() {
            if socket.write(Message::Text(text.into())).is_err() {
                failed = true;
                break;
            }
        }
        if failed || socket.flush().is_err() {
            break;
        }
        match socket.read() {
            Ok(Message::Text(text)) => handle_text(&text, shared, sink),
            Ok(Message::Binary(_) | Message::Ping(_) | Message::Pong(_) | Message::Frame(_)) => {}
            Ok(Message::Close(_)) => break,
            Err(tungstenite::Error::Io(error))
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(_) => break,
        }
    }
    fail_pending(shared);
}

/// Marks the connection terminal and answers every waiter
/// [`Error::Closed`]. Calls arriving later see the flag and fail fast.
fn fail_pending(shared: &Shared) {
    let mut pending = shared.pending.lock();
    pending.closed = true;
    for (_, tx) in pending.waiters.drain() {
        let _ = tx.send(Err(Error::Closed));
    }
}

fn handle_text(text: &str, shared: &Shared, sink: &Sender<StreamEvent>) {
    let Ok(Value::Object(mut frame)) = serde_json::from_str::<Value>(text) else {
        shared.malformed_frames.fetch_add(1, Ordering::Relaxed);
        return;
    };
    // A response carries `result` or `error`; a frame with `method`
    // is a notification or a request the VM forwards for a registered
    // service — which this client never registers, and whose id lives
    // in another client's space, so it must not be routed as an answer.
    if frame.contains_key("method") {
        if frame.get("method").and_then(Value::as_str) == Some("streamNotify") {
            handle_event(frame.remove("params"), shared, sink);
        }
        return;
    }
    handle_response(frame, shared);
}

fn handle_response(mut frame: serde_json::Map<String, Value>, shared: &Shared) {
    let id = match frame.get("id") {
        // We always send string ids; tolerate a numeric echo.
        Some(Value::String(text)) => text.parse::<u64>().ok(),
        Some(Value::Number(number)) => number.as_u64(),
        _ => None,
    };
    let Some(id) = id else {
        shared.malformed_frames.fetch_add(1, Ordering::Relaxed);
        return;
    };
    let Some(waiter) = shared.pending.lock().waiters.remove(&id) else {
        return; // timed-out or unknown id — discard
    };
    let answer = if let Some(error) = frame.get("error") {
        Err(Error::Rpc {
            code: error.get("code").and_then(Value::as_i64).unwrap_or(0),
            message: error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown rpc error")
                .to_owned(),
            details: error
                .get("data")
                .and_then(|data| data.get("details"))
                .and_then(Value::as_str)
                .map(str::to_owned),
        })
    } else {
        Ok(frame.remove("result").unwrap_or(Value::Null))
    };
    let _ = waiter.send(answer);
}

fn handle_event(params: Option<Value>, shared: &Shared, sink: &Sender<StreamEvent>) {
    let Some(Value::Object(mut params)) = params else {
        shared.malformed_frames.fetch_add(1, Ordering::Relaxed);
        return;
    };
    let raw = params.remove("event").unwrap_or(Value::Null);
    // Deserialize from a borrow: the raw JSON is kept without a copy.
    let Ok(event) = Event::deserialize(&raw) else {
        shared.malformed_frames.fetch_add(1, Ordering::Relaxed);
        return;
    };
    let stream_id = match params.remove("streamId") {
        Some(Value::String(stream_id)) => stream_id,
        _ => String::new(),
    };
    // Only this thread writes `event_seq`/`settled_events`.
    let seq = shared.event_seq.fetch_add(1, Ordering::Relaxed);
    let entry = StreamEvent {
        seq,
        stream_id,
        event,
        raw,
    };
    // Bounded feed: a stalled consumer drops the newest entry rather
    // than blocking the connection thread; the event's sequence number
    // stays consumed, so the hole is exact.
    if sink.try_send(entry).is_err() {
        shared.dropped_events.fetch_add(1, Ordering::Relaxed);
    }
    shared.settled_events.store(seq + 1, Ordering::Release);
}

fn check_not_sentinel(value: &Value) -> Result<()> {
    if value.get("type").and_then(Value::as_str) == Some("Sentinel") {
        let kind = value
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("Unknown")
            .to_owned();
        let value_as_string = value
            .get("valueAsString")
            .and_then(Value::as_str)
            .map(str::to_owned);
        return Err(Error::Sentinel {
            kind,
            value_as_string,
        });
    }
    Ok(())
}
