//! Structured errors for the debug session. Every failure mode is a
//! typed variant — the session never panics on a reachable path; bad
//! wire data, RPC errors and process failures are all user-visible and
//! recoverable.

/// The crate's result alias.
pub type Result<T> = std::result::Result<T, Error>;

/// Everything that can fail between spawning a Dart process and reading
/// a variable out of a paused frame.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A local IO failure (spawning the process, reading its pipes).
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// The WebSocket transport itself failed (handshake or frame IO).
    #[error("websocket error: {0}")]
    WebSocket(#[from] tungstenite::Error),

    /// The wire shape was legal JSON but violated the protocol: missing
    /// or mistyped required fields, unparseable frames. The string names
    /// the violated expectation.
    #[error("protocol violation: {0}")]
    Protocol(String),

    /// The VM answered an RPC with a JSON-RPC error object. `code` is
    /// the service-protocol application code (100–115) or a JSON-RPC
    /// code; `details` carries the optional `data.details` text.
    #[error("rpc error {code}: {message}")]
    Rpc {
        /// The numeric error code.
        code: i64,
        /// The `message` member.
        message: String,
        /// The optional `data.details` member.
        details: Option<String>,
    },

    /// The peer's protocol major version differs from the one this
    /// client implements (major bumps are, by spec, potentially
    /// incompatible changes).
    #[error(
        "incompatible protocol version {found_major}.{found_minor}; expected major {expected_major}"
    )]
    VersionIncompatible {
        /// The major version this client speaks.
        expected_major: u64,
        /// The reported major version.
        found_major: u64,
        /// The reported minor version.
        found_minor: u64,
    },

    /// The VM returned a sentinel where a real object was required —
    /// `Collected` (GC'd or dead isolate), `Expired`, `NotInitialized`,
    /// `OptimizedOut`, etc.
    #[error("sentinel response: {kind}")]
    Sentinel {
        /// The sentinel kind as spelled on the wire.
        kind: String,
        /// The sentinel's `valueAsString`, when present.
        value_as_string: Option<String>,
    },

    /// The `dart` executable could not be located (neither
    /// `DARTQ_DART_BIN` nor `PATH` produced one).
    #[error("dart binary not found: {0}")]
    DartBinaryNotFound(String),

    /// The `flutter` executable could not be located (neither
    /// `DARTQ_FLUTTER_BIN` nor `PATH` produced one).
    #[error("flutter binary not found: {0}")]
    FlutterBinaryNotFound(String),

    /// The spawned process did not print its service URI banner before
    /// the launch deadline elapsed.
    #[error("timed out waiting for the VM service URI banner")]
    LaunchTimeout,

    /// The process printed output but no line matched the VM service
    /// URI banner shape.
    #[error("could not parse the VM service URI from process output: {0}")]
    BannerParse(String),

    /// The process exited before the service connection was
    /// established or while the session was active.
    #[error("dart process exited early (exit code {0:?})")]
    ProcessExited(Option<i32>),

    /// A local deadline elapsed: an RPC round-trip, a full write queue,
    /// or a wait for the isolate to pause. Distinct from [`Error::Rpc`]
    /// so callers can tell "the VM refused" from "we stopped waiting".
    #[error("timed out: {0}")]
    Timeout(String),

    /// The isolate a call was waiting on exited.
    #[error("isolate {0} exited")]
    IsolateExited(String),

    /// The connection was closed — either the peer hung up or the
    /// session was shut down; pending RPCs are answered with this.
    #[error("connection closed")]
    Closed,
}
