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
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender, bounded};
use rustc_hash::FxHashMap;
use serde_json::{Value, json};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

use crate::error::{Error, Result};
use crate::protocol::Event;

/// Upper bound on one inbound message body. VM responses carry whole
/// scripts and stack dumps; 64 MiB is comfortably above real traffic
/// while still bounding a broken peer's allocation blast radius.
const MAX_MESSAGE_BYTES: usize = 64 << 20;

/// Default timeout for a single RPC round-trip. VM operations that
/// can take longer (reload of a large app) lift it per call.
const DEFAULT_RPC_TIMEOUT: Duration = Duration::from_secs(30);

/// How often the connection thread wakes to drain the write channel
/// when no inbound frame arrives.
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

/// How many stream events the connection buffers. Consumers drain via
/// [`VmClient::drain_events`]/[`VmClient::next_event`]; overruns drop
/// the oldest entries so a stalled consumer cannot grow memory or
/// block the reader.
pub const EVENT_BUFFER_CAPACITY: usize = 10_000;

/// How many outbound request texts may wait unwritten. Writes drain
/// every [`READ_TICK`]; 1024 is far above real burst rates.
const WRITE_QUEUE_CAPACITY: usize = 1024;

type PendingMap = FxHashMap<u64, Sender<std::result::Result<Value, Error>>>;

/// One write command for the connection thread — request text or a
/// close request.
#[derive(Debug)]
enum Wire {
    Text(String),
    Close,
}

struct Shared {
    pending: Mutex<PendingMap>,
    next_id: AtomicU64,
    /// Sequence numbering for stream events (shared by the event
    /// ring and channel payload).
    event_seq: AtomicU64,
    /// Events dropped because the bounded event feed was full —
    /// consumers surface the gap through [`VmClient::dropped_events`].
    dropped_events: AtomicU64,
    /// Live [`VmClient`] handles; the connection thread exits when
    /// the last one drops and nothing is in flight.
    handles: AtomicUsize,
    /// The connection thread, joined once by the last dropping handle
    /// so a dropped client never leaves the thread/socket alive.
    conn_thread: Mutex<Option<JoinHandle<()>>>,
}

/// One decoded stream event plus its sequence number in the
/// connection's event feed.
#[derive(Debug, Clone)]
pub struct StreamEvent {
    /// Monotonic per-connection sequence number.
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
    writes: Sender<Wire>,
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
        let (socket, _response) = tungstenite::connect(ws_uri)?;
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
        set_read_timeout(&mut socket)?;
        let (event_sink, events) = bounded(EVENT_BUFFER_CAPACITY);
        let (writes, write_rx) = bounded(WRITE_QUEUE_CAPACITY);
        let shared = Arc::new(Shared {
            pending: Mutex::new(FxHashMap::default()),
            next_id: AtomicU64::new(1),
            event_seq: AtomicU64::new(0),
            dropped_events: AtomicU64::new(0),
            handles: AtomicUsize::new(1),
            conn_thread: Mutex::new(None),
        });
        let thread_shared = Arc::clone(&shared);
        let conn_thread = std::thread::Builder::new()
            .name("dartq-debug-conn".into())
            .spawn(move || connection_loop(&mut socket, &thread_shared, &write_rx, &event_sink))
            .map_err(Error::Io)?;
        // Fresh mutex — poisoning is impossible here.
        *shared.conn_thread.lock().expect("conn_thread mutex poisoned") = Some(conn_thread);
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
    /// [`Error::Rpc`] when the VM returns a JSON-RPC error object or
    /// the round-trip exceeds `timeout`; [`Error::Closed`] if the
    /// connection drops mid-flight.
    pub fn call(&self, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        let id = self.shared.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = bounded(1);
        self.shared
            .pending
            .lock()
            .map_err(|_| Error::Closed)?
            .insert(id, tx);
        let request = json!({
            "jsonrpc": "2.0",
            "id": id.to_string(),
            "method": method,
            "params": params,
        });
        if self.writes.try_send(Wire::Text(request.to_string())).is_err() {
            self.shared
                .pending
                .lock()
                .map_err(|_| Error::Closed)?
                .remove(&id);
            return Err(Error::Closed);
        }
        match rx.recv_timeout(timeout) {
            Ok(answer) => answer,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                // Leave the waiter registered: a late answer is routed
                // and dropped by the (already disconnected) receiver
                // instead of stranding the id forever.
                Err(Error::Rpc {
                    code: -1,
                    message: format!("rpc {method} timed out after {timeout:?}"),
                    details: None,
                })
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => Err(Error::Closed),
        }
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
    /// gone); `Ok(None)` on timeout.
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

    /// Closes the connection: pending RPCs answer [`Error::Closed`]
    /// and the connection thread exits at the next tick.
    pub fn close(&self) {
        let _ = self.writes.try_send(Wire::Close);
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
        // The last handle drops the write side's close request and
        // joins the connection thread — the read tick bounds the wait.
        if self.shared.handles.fetch_sub(1, Ordering::AcqRel) == 1 {
            let _ = self.writes.try_send(Wire::Close);
            let thread = self
                .shared
                .conn_thread
                .lock()
                .ok()
                .and_then(|mut slot| slot.take());
            if let Some(thread) = thread {
                let _ = thread.join();
            }
        }
    }
}

/// Applies the connection thread's read timeout to the transport
/// underneath the WebSocket.
fn set_read_timeout(socket: &mut WebSocket<MaybeTlsStream<TcpStream>>) -> Result<()> {
    match socket.get_mut() {
        MaybeTlsStream::Plain(stream) => stream.set_read_timeout(Some(READ_TICK)),
        MaybeTlsStream::Rustls(stream) => stream.sock.set_read_timeout(Some(READ_TICK)),
        _ => {
            return Err(Error::Protocol(
                "unsupported WebSocket transport for read tick".to_owned(),
            ));
        }
    }
    .map_err(Error::Io)
}

fn connection_loop(
    socket: &mut WebSocket<MaybeTlsStream<TcpStream>>,
    shared: &Shared,
    writes: &Receiver<Wire>,
    sink: &Sender<StreamEvent>,
) {
    loop {
        // Drain queued writes first: requests must hit the wire even
        // when the peer is silent.
        while let Ok(wire) = writes.try_recv() {
            match wire {
                Wire::Text(text) => {
                    if socket.send(Message::Text(text.into())).is_err() {
                        fail_pending(shared);
                        return;
                    }
                }
                Wire::Close => {
                    let _ = socket.close(None);
                    let _ = socket.flush();
                    fail_pending(shared);
                    return;
                }
            }
        }
        let _ = socket.flush();
        match socket.read() {
            Ok(Message::Text(text)) => {
                if text.len() <= MAX_MESSAGE_BYTES {
                    handle_text(&text, shared, sink);
                }
            }
            Ok(Message::Binary(_) | Message::Ping(_) | Message::Pong(_)) => {}
            Ok(Message::Close(_) | Message::Frame(_)) => break,
            Err(tungstenite::Error::Io(error))
                if error.kind() == std::io::ErrorKind::WouldBlock
                    || error.kind() == std::io::ErrorKind::TimedOut => {}
            Err(_) => break,
        }
        if shared.handles.load(Ordering::Relaxed) == 0 && shared_pending_empty(shared) {
            // Every client handle dropped and nothing is in flight —
            // clean shutdown.
            break;
        }
    }
    fail_pending(shared);
}

fn shared_pending_empty(shared: &Shared) -> bool {
    shared.pending.lock().map(|p| p.is_empty()).unwrap_or(true)
}

fn fail_pending(shared: &Shared) {
    if let Ok(mut pending) = shared.pending.lock() {
        for (_, tx) in pending.drain() {
            let _ = tx.send(Err(Error::Closed));
        }
    }
}

fn handle_text(text: &str, shared: &Shared, sink: &Sender<StreamEvent>) {
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return; // unparseable frame — drop, keep the connection alive
    };
    let Some(object) = value.as_object() else {
        return;
    };
    if object.contains_key("id") {
        handle_response(value, shared);
        return;
    }
    if object.get("method").and_then(Value::as_str) == Some("streamNotify") {
        handle_event(value, shared, sink);
    }
    // Anything else (e.g. a registered-service forwarded request we do
    // not serve) is dropped — the VM only sends responses and
    // streamNotify to a client.
}

fn handle_response(value: Value, shared: &Shared) {
    let id = match value.get("id") {
        // We always send string ids (`id.to_string()`); tolerate a
        // numeric echo from a middleware peer.
        Some(Value::String(text)) => text.parse::<u64>().ok(),
        Some(Value::Number(number)) => number.as_u64(),
        _ => None,
    };
    let Some(id) = id else { return };
    let waiter = shared
        .pending
        .lock()
        .map(|mut table| table.remove(&id))
        .ok()
        .flatten();
    let Some(waiter) = waiter else {
        return; // timed-out or unknown id — discard
    };
    let answer = if let Some(error) = value.get("error") {
        let code = error.get("code").and_then(Value::as_i64).unwrap_or(0);
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("unknown rpc error")
            .to_owned();
        let details = error
            .get("data")
            .and_then(|d| d.get("details"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        Err(Error::Rpc {
            code,
            message,
            details,
        })
    } else {
        Ok(value.get("result").cloned().unwrap_or(Value::Null))
    };
    let _ = waiter.send(answer);
}

fn handle_event(value: Value, shared: &Shared, sink: &Sender<StreamEvent>) {
    let Some(params) = value.get("params") else { return };
    let stream_id = params
        .get("streamId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let raw = params.get("event").cloned().unwrap_or(Value::Null);
    let Ok(event) = serde_json::from_value::<Event>(raw.clone()) else {
        return; // malformed event — drop just this message
    };
    let seq = shared.event_seq.fetch_add(1, Ordering::Relaxed);
    let entry = StreamEvent {
        seq,
        stream_id,
        event,
        raw,
    };
    // Bounded feed: a stalled consumer drops the newest entry rather
    // than blocking the connection thread — the consumer learns of
    // the hole through `dropped_events`. A disconnected feed counts
    // too: the counter simply goes unread.
    if sink.try_send(entry).is_err() {
        shared.dropped_events.fetch_add(1, Ordering::Relaxed);
    }
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
