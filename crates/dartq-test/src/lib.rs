//! `dartq test`'s substrate: run `dart test` suites faster with output the
//! stock runner would print.
//!
//! - [`cache`] — content-addressed suite result cache: a suite whose
//!   import closure, run-time inputs, resolution, toolchain, arguments and
//!   environment hash to an earlier green run's key replays that run's
//!   reporter events. [`deps`] and [`stamp`] are its substrate (directive
//!   extraction on the dartq scanner, stat-indexed blake3 content stamps).
//! - [`group`] — the grouped runner: every suite compiled into one program
//!   and started as an isolate of its group under the stock runner.
//! - [`flutter`] — the grouped pass of `dartq test --flutter`: suites
//!   compiled in shared chunks, each run as a spawned engine of a reused
//!   `flutter_tester` under the stock runner.
//! - [`exec`] — runner invocation, per-suite event blocks, idle watchdog.
//! - [`host`] — host-wide memory leases shared by every dartq process of
//!   the user, runner process-group teardown, toolchain lookup.
//! - [`select`] — the runner's suite-level selection (`@Tags` against the
//!   tag selectors, `@TestOn` against the VM).
//! - [`report`] — the stock reporters, rendered from protocol events.
//! - [`event`] — the protocol event value.

pub mod cache;
pub mod deps;
pub mod event;
pub mod exec;
pub mod flutter;
pub mod group;
pub mod host;
pub mod report;
pub mod select;
pub mod stamp;

pub use cache::{Cache, Runner};
pub use exec::{ExecConfig, ExecError, Execution, Loose, Settled, SuiteBlock, execute, execute_flutter, execute_grouped};
pub use group::GroupRunner;
pub use host::{Lease, Profile, cache_dir, lease, lease_processes, lease_run, split};
