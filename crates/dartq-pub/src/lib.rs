//! Consumers for pub's resolution artifacts — `package_config.json`,
//! `pubspec.yaml`, `pubspec.lock` — plus workspace discovery, freshness,
//! and [`graph::ResolvedPackageGraph`] assembly.
//!
//! dartq does **no version solving**: `pub get` is an external dependency,
//! and this crate only reads what it materialized. Behavior is anchored to
//! the pub formats contract, which cites upstream pub line-by-line.
//!
//! Module map:
//! - [`version`] module — pub_semver subset and language versions
//! - [`config`] module — `package_config.json`
//! - [`pubspec`] module — pubspec consumption field set
//! - [`lock`] module — `pubspec.lock` with the four source shapes
//! - [`workspace`]: workspace root walk and `workspace_ref.json` checks
//! - [`freshness`]: staleness judgment — mtime chain, environment
//!   fingerprint, and the content-id fallback; reports reasons, never
//!   diagnostics directly
//! - [`glob`]: the pinned `package:glob` 2.1.3 port (parse + full-path match
//!   + listing), shared by workspace expansion and analysis-options excludes
//! - [`graph`]: `ResolvedPackageGraph` / `PackageGraphId`
//!
//! Docs throughout cite upstream pub source paths and measured facts from
//! the pub formats contract verbatim; do not lint-prose them.
#![allow(clippy::assert_is_empty, clippy::semicolon_if_nothing_returned)]
#![allow(clippy::too_many_lines)]
#![allow(clippy::type_complexity)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::used_underscore_binding)]
#![allow(clippy::format_push_string)]
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
#![allow(clippy::while_let_loop)]
#![allow(clippy::if_same_then_else)]
#![allow(clippy::ref_option)]
#![allow(clippy::case_sensitive_file_extension_comparisons)]
#![allow(clippy::unnecessary_filter_map)]
#![allow(clippy::manual_strip)]
#![allow(clippy::float_cmp)]
#![allow(clippy::redundant_guards)]
#![allow(clippy::empty_line_after_doc_comments)]
#![allow(clippy::unreadable_literal)]
#![allow(clippy::enum_variant_names)]
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::unnecessary_wraps)]
#![allow(clippy::too_many_arguments)]
#![allow(clippy::unused_self)]
#![allow(clippy::fn_params_excessive_bools)]
#![allow(elided_lifetimes_in_paths)]
#![allow(clippy::suspicious_open_options)]
#![allow(clippy::cloned_ref_to_slice_refs)]
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
#![allow(clippy::doc_markdown)]

pub mod config;
pub mod freshness;
mod git;
pub mod glob;
pub mod graph;
pub mod lock;
pub mod pubspec;
pub mod pubspec_facts;
pub mod universe;
pub mod version;
pub mod workspace;

pub use config::{PackageConfig, PackageConfigEntry, ResolvedPackage};
pub use freshness::{EnvSnapshot, Freshness, FreshnessReport, ResolutionBaseline, StaleReason};
pub use graph::{
    DepKind, PackageEdge, PackageGraphId, PackageInstance, PackageInstanceId, ResolvedPackageGraph,
    SDK_INSTANCE_NAME, SourceRef, build_graph,
};
pub use lock::{DependencyKind, LockDescription, LockFile, LockedPackage, SourceKind};
pub use pubspec::{DependencySpec, Environment, Pubspec, Resolution};
pub use pubspec_facts::{PubspecFacts, Scalar};
pub use universe::{
    BoundaryEntry, BoundaryReason, PackageEntry, PackageKind, ScanOptions, ScanReport, Universe,
    enclosing_universe, scan,
};
pub use version::{LanguageVersion, Version, VersionConstraint, VersionRange};
pub use workspace::{WorkspaceLayout, find_workspace_root, normalize, workspace_ref_mismatches};

/// Errors from parsing or consuming pub artifacts.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PubError {
    /// `package_config.json` is invalid JSON or violates the schema.
    #[error("package_config.json: {0}")]
    PackageConfig(String),
    /// `pubspec.yaml` (or `pubspec_overrides.yaml`) is malformed.
    #[error("pubspec.yaml: {0}")]
    Pubspec(String),
    /// `pubspec.lock` is malformed.
    #[error("pubspec.lock: {0}")]
    LockFile(String),
    /// A version or constraint string is outside the pub_semver subset.
    #[error("version: {0}")]
    Version(String),
    /// Workspace discovery found no pubspec anywhere on the walk — the
    /// typed face of the "Found no `pubspec.yaml`" case.
    #[error("workspace: {0}")]
    NoRoot(String),
    /// Workspace discovery met a `resolution: workspace` pubspec that no
    /// workspace root above it includes — pub's refusal, but the
    /// analyzer's floor: it roots the context at the nearest pubspec and
    /// analyzes (the oracle never consults pub's root walk).
    #[error("workspace: {0}")]
    OrphanMember(String),
    /// Workspace discovery or artifact validation failed.
    #[error("workspace: {0}")]
    Workspace(String),
    /// Graph assembly failed (canonical identity serialization).
    #[error("package graph: {0}")]
    Graph(String),
}
