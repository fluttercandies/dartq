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
//! * [`resolve`] — the position faces' shared core: `resolveAt` (the
//!   symbol under one offset) and `completion` (the visible-name set at
//!   one cursor), the daemon RPC answers and the CLI one-shot subcommands'
//!   one implementation.
//! * [`rename`] — the symbol rename face: the anchor resolution joined
//!   with the reference rows, as a machine-readable edit set plus the
//!   drift-checked patcher.
//! * [`explain`] — the explain report.
//! * [`impact`] — the impact report.
//! * [`compat`] — the per-SDK compatibility matrix.
//! * [`fix`] — the anchored patch substrate.
//! * [`cache`] — the `cache stats|gc` aggregate over every on-disk cache:
//!   the build cache, the facts snapshot store, and the content store.
//! * [`lints`] — the lint face's mount: enable-set resolution
//!   from the merged analysis options and lint rows as engine diagnostics.
//! * [`plugins`] — the analyze face's plugin cycle (launch, feed, drain,
//!   findings→diagnostic merge) over `dartq-plugin`, the CLI `dart analyze`
//!   counterpart of the daemon's plugin weave.
//! * [`uri`] — the `file:` URI ↔ path mapping (the daemon's overlay keys,
//!   the analyze faces' document URIs).
//! * `pkg_label` — the output faces' file references: the lon/text file
//!   locator ([`locate`], relative-inside-root/absolute-outside) and the
//!   frozen package-form reference-label grammar the exchange faces print.
//!
//! [`EngineConfig`]: dartq_engine::EngineConfig

#![allow(clippy::format_push_string, clippy::semicolon_if_nothing_returned)]
#![allow(clippy::too_many_lines)]
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::unnecessary_wraps)]
#![allow(clippy::too_many_arguments)]
#![allow(clippy::type_complexity)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::assert_is_empty)]
#![allow(clippy::used_underscore_binding)]
#![allow(clippy::manual_let_else)]
#![allow(clippy::default_trait_access)]
#![allow(clippy::ptr_arg)]
#![allow(clippy::needless_pass_by_value)]
#![allow(clippy::match_same_arms)]
#![allow(clippy::items_after_statements)]
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
#![allow(clippy::unnecessary_unwrap)]
#![allow(clippy::assigning_clones)]
#![allow(clippy::double_must_use)]
#![allow(clippy::single_match)]
#![allow(clippy::cast_possible_wrap)]
#![allow(clippy::cast_precision_loss)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::ignore_without_reason)]
#![allow(clippy::unnecessary_debug_formatting)]
#![allow(clippy::match_wild_err_arm)]
#![allow(clippy::disallowed_names)]
#![allow(clippy::many_single_char_names)]
#![allow(clippy::same_item_push)]
#![allow(clippy::empty_line_after_doc_comments)]
#![allow(clippy::unreadable_literal)]
#![allow(clippy::enum_variant_names)]
#![allow(clippy::unnecessary_sort_by)]
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
#![allow(clippy::unused_async)]
#![allow(clippy::used_underscore_items)]
#![allow(clippy::struct_excessive_bools)]
#![allow(clippy::struct_field_names)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::redundant_pub_crate)]
#![allow(clippy::must_use_candidate)]
#![allow(
    clippy::doc_markdown,
    clippy::while_let_loop,
    clippy::if_same_then_else,
    clippy::ref_option,
    clippy::case_sensitive_file_extension_comparisons,
    clippy::unnecessary_filter_map,
    clippy::manual_strip,
    clippy::float_cmp,
    clippy::redundant_guards,
    elided_lifetimes_in_paths,
    clippy::suspicious_open_options
)]

pub use pkg_label::locate;

pub mod analyze;
pub mod bcache;
pub mod cache;
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
pub mod packages;
mod pkg_label;
pub mod plugins;
pub mod query;
pub mod rename;
pub mod render;
pub mod resolve;
mod rule_states;
pub mod uri;
pub mod workspace;

#[cfg(test)]
mod ticket_3580_tests;

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
