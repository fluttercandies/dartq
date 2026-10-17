//! The per-command cores shared by the CLI and the daemon RPC faces (the
//! one-implementation rule): plain parameters in, typed results
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
//! * [`check`] — the M0 scan+parse pipeline plus the relative-URI
//!   existence face (`uri_does_not_exist` for missing directive targets).
//! * [`config`] — the `dartq.yaml` workspace config plus the
//!   compat face's axis-root resolution over it.
//! * [`analyze`] — the full engine pass (discover + analyze + oracle sort).
//! * [`query`] — the symbol index query over the analyzed state.
//! * [`rename`] — the symbol rename face: the anchor resolution joined
//!   with the reference rows, as a machine-readable edit set plus the
//!   drift-checked patcher.
//! * [`explain`] — the explain report.
//! * [`impact`] — the impact report.
//! * [`compat`] — the per-SDK compatibility matrix.
//! * [`fix`] — the anchored patch substrate.
//! * [`lints`] — the lint face's mount: enable-set resolution
//!   from the merged analysis options and lint rows as engine diagnostics.
//! * [`plugins`] — the analyze face's plugin cycle (launch, feed, drain,
//!   findings→diagnostic merge) over `dartq-plugin`, the CLI `dart analyze`
//!   counterpart of the daemon's plugin weave.
//! * [`uri`] — the `file:` URI ↔ path mapping (the daemon's overlay keys,
//!   the analyze faces' document URIs).
//! * `pkg_label` — the stable package-form file reference the output
//!   faces print (the one implementation of the reference-label grammar).
//!
//! [`EngineConfig`]: dartq_engine::EngineConfig

pub mod analyze;
pub mod check;
pub mod compat;
pub mod config;
pub mod discover;
pub mod explain;
pub mod fix;
pub mod fix_table;
pub mod impact;
pub mod index;
pub mod layout;
mod lint_names;
pub mod lints;
mod pkg_label;
pub mod plugins;
pub mod query;
pub mod rename;
mod rule_states;
pub mod uri;
pub mod workspace;

use serde::Deserialize;

/// The response/output contract version. Mirrors the frozen v1 contract —
/// dartq-schema's per-domain constants (`SCHEMA_VERSION_IMPACT`,
/// `SCHEMA_VERSION_COMPAT`, `SCHEMA_VERSION_RUN_METRICS`,
/// `SCHEMA_VERSION_FIX_PATCH`, …) are authoritative; this constant is the
/// one spelling the daemon RPC envelopes stamp alongside faces whose
/// producer type carries no version field of its own.
pub const SCHEMA_VERSION: u32 = 1;

/// How much detail an output face carries. Default
/// [`Verbosity::Compact`].
///
/// The canonical tier semantics are pinned by the schema-freeze contract —
/// the "Verbosity tiers" section of `dartq-schema`'s crate docs is the
/// single source of truth (per-face field tables); do not invent tier
/// semantics here. Summary:
///
/// * `minimal` — the red-line floor: locating fields are never
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
