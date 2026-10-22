//! An in-process mock of the Dart VM Service endpoint.
//!
//! The mock accepts one WebSocket client on a loopback port, answers
//! each JSON-RPC request with a scripted [`Reply`], and can push
//! stream events either from the test thread ([`MockVm::control`]) or
//! as a prelude written *before* a response ([`Reply::Prefixed`]).
//!
//! The prelude ordering is what makes the tests deterministic without
//! sleeping: the client's connection thread processes frames in wire
//! order, so once the barrier RPC returns, every prelude event is
//! already queued in the client's event feed.

#![allow(dead_code)]

use std::io::ErrorKind;
use std::net::{TcpListener, TcpStream};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender, bounded};
use serde_json::{Value, json};
use tungstenite::{Message, WebSocket, accept};

/// How long a test waits for one expected request before failing.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

/// One request observed by the mock.
#[derive(Debug, Clone)]
pub struct Request {
    /// The `method` member.
    pub method: String,
    /// The `params` member.
    pub params: Value,
    /// The raw `id` member, echoed back in the response.
    pub id: Value,
}

/// A frame the mock writes outside the request/response flow.
#[derive(Debug, Clone)]
pub enum Control {
    /// A `streamNotify` event on `stream_id`.
    Event {
        /// The VM stream id.
        stream_id: String,
        /// The event payload.
        event: Value,
    },
    /// A raw text frame (for malformed-frame and stray-id tests).
    Raw(String),
    /// Close the WebSocket.
    Close,
}

/// A scripted answer to one request.
pub enum Reply {
    /// `{"result": value}`.
    Ok(Value),
    /// `{"error": {code, message, data}}`.
    Err {
        /// Application error code.
        code: i64,
        /// Error message.
        message: String,
        /// Optional `data.details` text.
        details: Option<String>,
    },
    /// Write nothing (used for timeout tests).
    Silent,
    /// Write the prelude frames, then the result.
    Prefixed {
        /// Frames written before the response.
        prelude: Vec<Control>,
        /// The result payload.
        result: Value,
    },
}

/// Builds an `Ok` reply.
pub fn ok(result: Value) -> Reply {
    Reply::Ok(result)
}

/// Builds an error reply with no `data` member.
pub fn err(code: i64, message: &str) -> Reply {
    Reply::Err {
        code,
        message: message.to_owned(),
        details: None,
    }
}

/// Builds an error reply carrying `data.details`.
pub fn err_with_details(code: i64, message: &str, details: &str) -> Reply {
    Reply::Err {
        code,
        message: message.to_owned(),
        details: Some(details.to_owned()),
    }
}

/// The protocol's empty `Success` response.
pub fn success() -> Reply {
    ok(json!({ "type": "Success" }))
}

/// A mock VM service listening on loopback.
pub struct MockVm {
    /// The WebSocket URI a client should connect to.
    pub ws_uri: String,
    control: Sender<Control>,
    /// Every request the mock received, in arrival order.
    pub requests: Receiver<Request>,
    handle: Option<JoinHandle<()>>,
}

impl MockVm {
    /// Starts a mock that answers requests with `handler`.
    pub fn start<H>(handler: H) -> Self
    where
        H: FnMut(&Request) -> Reply + Send + 'static,
    {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind mock listener");
        let port = listener.local_addr().expect("mock local address").port();
        let (control, control_rx) = bounded::<Control>(256);
        let (requests_tx, requests) = bounded::<Request>(1024);
        let mut handler = handler;
        let handle = thread::Builder::new()
            .name("mock-vm-service".to_owned())
            .spawn(move || {
                let Ok((stream, _)) = listener.accept() else {
                    return;
                };
                let Ok(socket) = accept(stream) else {
                    return;
                };
                serve(socket, &mut handler, &control_rx, &requests_tx);
            })
            .expect("spawn mock vm service thread");
        Self {
            ws_uri: format!("ws://127.0.0.1:{port}/ws"),
            control,
            requests,
            handle: Some(handle),
        }
    }

    /// Starts a mock with the standard handshake and RPC defaults.
    pub fn start_default() -> Self {
        Self::start(default_reply)
    }

    /// A handle for pushing frames from the test thread.
    pub fn control(&self) -> Sender<Control> {
        self.control.clone()
    }

    /// Waits for the next request with `method`, skipping unrelated
    /// requests (handshake traffic, barriers).
    pub fn expect_request(&self, method: &str) -> Request {
        loop {
            let request = self
                .requests
                .recv_timeout(REQUEST_TIMEOUT)
                .unwrap_or_else(|_| panic!("mock never received a `{method}` request"));
            if request.method == method {
                return request;
            }
        }
    }

    /// Collects `count` requests with `method`, in arrival order.
    pub fn expect_requests(&self, method: &str, count: usize) -> Vec<Request> {
        let mut found = Vec::with_capacity(count);
        while found.len() < count {
            let request = self
                .requests
                .recv_timeout(REQUEST_TIMEOUT)
                .unwrap_or_else(|_| panic!("mock never received `{method}` #{count}"));
            if request.method == method {
                found.push(request);
            }
        }
        found
    }
}

impl Drop for MockVm {
    fn drop(&mut self) {
        // Closing the socket unblocks the serve loop; a mock whose
        // client never connected is left to the test process exit.
        let _ = self.control.try_send(Control::Close);
        self.handle.take();
    }
}

/// The personality of a bare VM service: handshake, resume/pause,
/// breakpoint removal and pause-mode RPCs succeed; everything else is
/// `-32601`.
pub fn default_reply(request: &Request) -> Reply {
    match request.method.as_str() {
        "getVersion" => ok(json!({ "type": "Version", "major": 4, "minor": 23 })),
        "getVM" => ok(json!({
            "type": "VM",
            "name": "mock-vm",
            "architectureBits": 64,
            "pid": 4242,
            "isolates": [isolate_ref("isolates/1", "1", "main")],
            "systemIsolates": [],
        })),
        // A runnable, running isolate: no pause to seed at attach.
        "getIsolate" => ok(json!({
            "type": "Isolate",
            "id": request.params["isolateId"],
            "number": "1",
            "name": "main",
            "runnable": true,
            "pauseEvent": {
                "type": "Event",
                "kind": "Resume",
                "isolate": isolate_ref(
                    request.params["isolateId"].as_str().unwrap_or_default(),
                    "1",
                    "main",
                ),
                "timestamp": 1_i64,
            },
            "breakpoints": [],
        })),
        "streamListen" | "streamCancel" => success(),
        "resume" | "pause" => success(),
        "removeBreakpoint" | "setIsolatePauseMode" | "setExceptionPauseMode" => success(),
        "getStack" => ok(json!({
            "type": "Stack",
            "frames": [],
            "asyncCausalFrames": [],
            "truncated": false,
        })),
        "getScripts" => ok(json!({
            "type": "ScriptList",
            "scripts": [script_ref("scripts/1", "package:app/main.dart")],
        })),
        "getObject" => ok(json!({
            "type": "Instance",
            "id": "objects/0",
            "kind": "PlainInstance",
            "fields": [],
        })),
        "reloadSources" => ok(json!({ "type": "ReloadReport", "success": true })),
        other => err(-32601, &format!("method not found: {other}")),
    }
}

/// An `@Isolate` reference.
pub fn isolate_ref(id: &str, number: &str, name: &str) -> Value {
    json!({
        "type": "@Isolate",
        "id": id,
        "number": number,
        "name": name,
        "isSystemIsolate": false,
    })
}

/// A `@Script` reference.
pub fn script_ref(id: &str, uri: &str) -> Value {
    json!({ "type": "@Script", "id": id, "uri": uri })
}

/// An `@Instance` reference of one primitive kind.
pub fn instance_ref(id: &str, kind: &str, value: &str) -> Value {
    json!({
        "type": "@Instance",
        "id": id,
        "kind": kind,
        "valueAsString": value,
    })
}

/// A `SourceLocation` with line/column and its script.
pub fn source_location(script_id: &str, uri: &str, line: i64, column: i64) -> Value {
    json!({
        "type": "SourceLocation",
        "script": script_ref(script_id, uri),
        "tokenPos": line * 100 + column,
        "line": line,
        "column": column,
    })
}

/// A `PauseBreakpoint` event naming `breakpoints` as the hit list.
pub fn pause_breakpoint_event(isolate: &str, breakpoints: &[&str]) -> Value {
    let hits: Vec<Value> = breakpoints
        .iter()
        .map(|id| {
            json!({
                "type": "Breakpoint",
                "id": id,
                "breakpointNumber": 1,
                "enabled": true,
                "resolved": true,
                "location": source_location("scripts/1", "package:app/main.dart", 7, 3),
            })
        })
        .collect();
    json!({
        "type": "Event",
        "kind": "PauseBreakpoint",
        "isolate": isolate_ref(isolate, "1", "main"),
        "timestamp": 1_700_000_000_000_i64,
        "breakpoint": hits.first().cloned().unwrap_or(Value::Null),
        "pauseBreakpoints": hits,
        "topFrame": top_frame(),
    })
}

/// A minimal `Frame` for pause events.
pub fn top_frame() -> Value {
    json!({
        "type": "Frame",
        "index": 0,
        "function": { "type": "@Function", "id": "functions/1", "name": "main" },
        "location": source_location("scripts/1", "package:app/main.dart", 7, 3),
        "vars": [],
        "kind": "Regular",
    })
}

/// A `Stack` with one frame whose locals are `vars`.
pub fn stack_with_vars(vars: Value) -> Value {
    json!({
        "type": "Stack",
        "frames": [{
            "type": "Frame",
            "index": 0,
            "function": { "type": "@Function", "id": "functions/1", "name": "main" },
            "location": source_location("scripts/1", "package:app/main.dart", 7, 3),
            "vars": vars,
            "kind": "Regular",
        }],
        "asyncCausalFrames": [],
        "truncated": false,
    })
}

/// Wraps `event` in a `streamNotify` envelope.
fn event_frame(stream_id: &str, event: &Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": "streamNotify",
        "params": { "streamId": stream_id, "event": event },
    })
}

/// The serve loop: drains control frames, answers requests, relays
/// frames until the peer closes or the transport fails.
fn serve<H>(
    mut socket: WebSocket<TcpStream>,
    handler: &mut H,
    control: &Receiver<Control>,
    requests: &Sender<Request>,
) where
    H: FnMut(&Request) -> Reply,
{
    let _ = socket
        .get_mut()
        .set_read_timeout(Some(Duration::from_millis(10)));
    loop {
        while let Ok(frame) = control.try_recv() {
            if !write_control(&mut socket, &frame) {
                return;
            }
        }
        let message = match socket.read() {
            Ok(message) => message,
            Err(tungstenite::Error::Io(error))
                if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) =>
            {
                continue;
            }
            Err(_) => return,
        };
        match message {
            Message::Text(text) => {
                let Ok(value) = serde_json::from_str::<Value>(&text) else {
                    continue;
                };
                let request = Request {
                    method: value
                        .get("method")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    params: value.get("params").cloned().unwrap_or(Value::Null),
                    id: value.get("id").cloned().unwrap_or(Value::Null),
                };
                let _ = requests.try_send(request.clone());
                match handler(&request) {
                    Reply::Ok(result) => {
                        if !write_result(&mut socket, &request.id, &result) {
                            return;
                        }
                    }
                    Reply::Err {
                        code,
                        message,
                        details,
                    } => {
                        if !write_error(&mut socket, &request.id, code, &message, details.as_deref())
                        {
                            return;
                        }
                    }
                    Reply::Silent => {}
                    Reply::Prefixed { prelude, result } => {
                        for frame in prelude {
                            if !write_control(&mut socket, &frame) {
                                return;
                            }
                        }
                        if !write_result(&mut socket, &request.id, &result) {
                            return;
                        }
                    }
                }
            }
            Message::Close(_) => {
                let _ = socket.close(None);
                return;
            }
            _ => {}
        }
    }
}

/// Writes one control frame; `false` means the connection ended.
fn write_control(socket: &mut WebSocket<TcpStream>, frame: &Control) -> bool {
    match frame {
        Control::Event { stream_id, event } => {
            let envelope = event_frame(stream_id, event);
            socket
                .send(Message::Text(envelope.to_string().into()))
                .is_ok()
        }
        Control::Raw(text) => socket
            .send(Message::Text(text.clone().into()))
            .is_ok(),
        Control::Close => {
            let _ = socket.close(None);
            false
        }
    }
}

/// Writes one successful response.
fn write_result(socket: &mut WebSocket<TcpStream>, id: &Value, result: &Value) -> bool {
    let response = json!({ "jsonrpc": "2.0", "id": id, "result": result });
    socket
        .send(Message::Text(response.to_string().into()))
        .is_ok()
}

/// Writes one error response.
fn write_error(
    socket: &mut WebSocket<TcpStream>,
    id: &Value,
    code: i64,
    message: &str,
    details: Option<&str>,
) -> bool {
    let error = details.map_or_else(
        || json!({ "code": code, "message": message }),
        |details| json!({ "code": code, "message": message, "data": { "details": details } }),
    );
    let response = json!({ "jsonrpc": "2.0", "id": id, "error": error });
    socket
        .send(Message::Text(response.to_string().into()))
        .is_ok()
}
