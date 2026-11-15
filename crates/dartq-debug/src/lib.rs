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

#![allow(clippy::type_complexity, clippy::semicolon_if_nothing_returned)]
#![allow(clippy::match_same_arms)]
#![allow(clippy::items_after_statements)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::too_many_lines)]
#![allow(clippy::too_many_arguments)]
#![allow(clippy::assert_is_empty)]
#![allow(clippy::used_underscore_binding)]
#![allow(clippy::format_push_string)]
#![allow(clippy::manual_let_else)]
#![allow(clippy::default_trait_access)]
#![allow(clippy::ptr_arg)]
#![allow(clippy::needless_pass_by_value)]
#![allow(clippy::similar_names)]
#![allow(clippy::single_match_else)]
#![allow(clippy::doc_lazy_continuation)]
#![allow(clippy::unnecessary_get_then_check)]
#![allow(clippy::match_wildcard_for_single_variants)]
#![allow(clippy::naive_bytecount)]
#![allow(clippy::sliced_string_as_bytes)]
#![allow(clippy::nonminimal_bool)]
#![allow(clippy::useless_conversion)]
#![allow(clippy::question_mark)]
#![allow(clippy::option_option)]
#![allow(clippy::needless_continue)]
#![allow(clippy::missing_panics_doc)]
#![allow(clippy::trivially_copy_pass_by_ref)]
#![allow(clippy::explicit_counter_loop)]
#![allow(clippy::never_loop)]
#![allow(clippy::needless_range_loop)]
#![allow(dead_code)]
#![allow(clippy::drop_non_drop)]
#![allow(clippy::wildcard_imports)]
#![allow(clippy::enum_glob_use)]
#![allow(clippy::if_not_else)]
#![allow(clippy::implicit_hasher)]
#![allow(clippy::iter_not_returning_iterator)]
#![allow(clippy::let_underscore_untyped)]
#![allow(clippy::map_unwrap_or)]
#![allow(clippy::missing_const_for_fn)]
#![allow(clippy::needless_raw_string_hashes)]
#![allow(clippy::needless_return)]
#![allow(clippy::option_if_let_else)]
#![allow(clippy::redundant_closure_for_method_calls)]
#![allow(clippy::redundant_else)]
#![allow(clippy::redundant_field_names)]
#![allow(clippy::redundant_static_lifetimes)]
#![allow(clippy::uninlined_format_args)]
#![allow(clippy::unnecessary_wraps)]
#![allow(clippy::unused_async)]
#![allow(clippy::used_underscore_items)]
#![allow(clippy::struct_excessive_bools)]
#![allow(clippy::struct_field_names)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::redundant_pub_crate)]
#![allow(clippy::must_use_candidate)]
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::doc_markdown)]
#![allow(clippy::cast_possible_wrap)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::cast_precision_loss)]
#![allow(
    clippy::double_must_use,
    clippy::unnecessary_unwrap,
    clippy::assigning_clones
)]

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
    Isolate, IsolateRef, LibraryRef, MapAssociation, ReloadReport, ScriptList, ScriptRef, Sentinel,
    SentinelKind, SourceLocation, Stack, StepOption, Success, UnresolvedSourceLocation,
    ValueOrSentinel, Version, Vm, VmObject,
};
pub use ring::Ring;
pub use session::{
    BreakpointHandle, BreakpointLocationSpec, BreakpointSpec, ConditionalResumeLimit, DebugSession,
    EventBatch, ExpandLimits, FrameLocation, FrameSummary, LaunchConfig, LaunchKind, OutputBatch,
    RELEASE_RESUME_BUDGET, RestartOutcome, SessionConfig, SessionEvent, StoppedReason,
    StoppedSnapshot, Variable,
};
