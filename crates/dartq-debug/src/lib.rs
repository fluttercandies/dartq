//! A synchronous client for the Dart VM Service Protocol plus the
//! AI-first debug session orchestration built on top of it.
//!
//! The crate speaks the protocol described by the upstream document
//! "Dart VM Service Protocol" (`runtime/vm/service/service.md` in the
//! Dart SDK source tree) over a blocking WebSocket connection:
//! one dedicated reader thread per connection matches JSON-RPC response
//! ids against a pending-request table and forwards stream events into a
//! bounded channel, while client calls write request frames through the
//! same connection.
//!
//! [`session::DebugSession`] layers the AI-first surface on top:
//! launching or attaching to a Dart process (a `dart` script or a
//! `flutter run` app — Flutter is dartq's primary target platform),
//! breakpoints (including conditional breakpoints implemented as
//! frame-scoped evaluation), step-and-package stopped snapshots, lazy
//! object-graph expansion by handle, exception pause mode, hot
//! reload/restart, and bounded stdout/stderr capture — all without
//! any IDE in the loop.

mod client;
mod error;
mod protocol;
mod ring;
mod session;

pub use client::{EVENT_BUFFER_CAPACITY, StreamEvent, VmClient};
pub use error::{Error, Result};
pub use protocol::{
    BoundField, BoundVariable, Breakpoint, BreakpointLocation, ClassRef, Event, EventKind,
    ExceptionPauseMode, Frame, FrameKind, FunctionRef, Instance, InstanceKind, InstanceRef,
    Isolate, IsolateRef, LibraryRef, MapAssociation, ReloadReport, ScriptList, ScriptRef,
    Sentinel, SentinelKind, SourceLocation, Stack, StepOption, Success, UnresolvedSourceLocation,
    ValueOrSentinel, Version, Vm, VmObject,
};
pub use ring::Ring;
pub use session::{
    BreakpointHandle, BreakpointLocationSpec, BreakpointSpec, ConditionalResumeLimit, DebugSession,
    EventBatch, ExpandLimits, FrameLocation, FrameSummary, LaunchConfig, LaunchKind, OutputBatch,
    RestartOutcome, SessionConfig, SessionEvent, StoppedReason, StoppedSnapshot, Variable,
};
