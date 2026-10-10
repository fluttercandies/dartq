//! The per-command cores shared by the CLI and the daemon RPC faces (PRD
//! §15.1's one-implementation rule): plain parameters in, typed results
//! out. No argument parsing, no printing, no process exit, no exit codes —
//! `dartq-cli` adds those on top (clap, output formats, exit tiers), and
//! `dartq-daemon` shapes the same results into `dartq/*` JSON-RPC
//! responses.
//!
//! # Faces
//!
//! * [`discover`] — the canonical `.dart` file-set walk (shared by the
//!   check and fix faces).
//! * [`workspace`] — the workspace/SDK assembly behind every engine-backed
//!   face ([`EngineConfig`] plus the canonical package directory).
//! * [`check`] — the M0 scan+parse pipeline.
//! * [`config`] — the `dartq.yaml` workspace config (PRD §15.9) plus the
//!   compat face's axis-root resolution over it.
//! * [`analyze`] — the full engine pass (discover + analyze + oracle sort).
//! * [`query`] — the symbol index query over the analyzed state.
//! * [`impact`] — the §15.4 impact report.
//! * [`compat`] — the §15.5 per-SDK compatibility matrix.
//! * [`fix`] — the §15.6 anchored patch substrate.
//!
//! [`EngineConfig`]: dartq_engine::EngineConfig

pub mod analyze;
pub mod check;
pub mod compat;
pub mod config;
pub mod discover;
pub mod fix;
pub mod impact;
pub mod index;
pub mod layout;
pub mod query;
pub mod workspace;

use serde::Deserialize;

/// The response/output contract version. Mirrors the frozen v1 contract —
/// dartq-schema's per-domain constants (`SCHEMA_VERSION_IMPACT`,
/// `SCHEMA_VERSION_COMPAT`, `SCHEMA_VERSION_RUN_METRICS`,
/// `SCHEMA_VERSION_FIX_PATCH`, …) are authoritative; this constant is the
/// one spelling the daemon RPC envelopes stamp alongside faces whose
/// producer type carries no version field of its own.
pub const SCHEMA_VERSION: u32 = 1;

/// How much detail an output face carries (PRD §15.1, §15.8.1). Default
/// [`Verbosity::Compact`].
///
/// The canonical tier semantics are pinned by the schema-freeze contract —
/// the "Verbosity tiers" section of `dartq-schema`'s crate docs is the
/// single source of truth (per-face field tables); do not invent tier
/// semantics here. Summary:
///
/// * `minimal` — the red-line floor (§15.8): locating fields are never
///   omissible (`code`/`file`/`range` or the face's equivalent); prose and
///   convenience spellings are dropped, never location;
/// * `compact` — the shape each producer emits today (the default; the
///   baseline every fixture pins);
/// * `full` — compact plus the per-face extras the canonical tables list
///   (today that is nothing beyond compact for the implemented faces).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Verbosity {
    /// Red-line fields only (code / file / range or the face's equivalent).
    Minimal,
    /// Today's output shape (the default).
    #[default]
    Compact,
    /// [`Verbosity::Compact`] plus the canonical tables' extras.
    Full,
}
