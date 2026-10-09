//! Consumers for pub's resolution artifacts — `package_config.json`,
//! `pubspec.yaml`, `pubspec.lock` — plus workspace discovery, freshness,
//! and [`graph::ResolvedPackageGraph`] assembly (PRD §8.2).
//!
//! dartq does **no version solving**: `pub get` is an external dependency,
//! and this crate only reads what it materialized. Behavior is anchored to
//! `docs/facts/pub-formats.md`, which cites `.third/pub` line-by-line.
//!
//! Module map:
//! - [`version`] module — pub_semver subset and language versions (§5)
//! - [`config`] module — `package_config.json` (§1)
//! - [`pubspec`] module — pubspec consumption field set (§2)
//! - [`lock`] module — `pubspec.lock` with the four source shapes (§3)
//! - [`workspace`]: workspace root walk and `workspace_ref.json` checks (§4)
//! - [`freshness`]: staleness judgment — mtime chain, environment
//!   fingerprint, and the content-id fallback (§6); reports reasons, never
//!   diagnostics directly
//! - [`glob`]: the pinned `package:glob` 2.1.3 port (parse + full-path match
//!   + listing), shared by workspace expansion and analysis-options excludes
//! - [`graph`]: `ResolvedPackageGraph` / `PackageGraphId`
//!
//! Docs throughout cite `.third/pub` source paths and measured facts from
//! `docs/facts/pub-formats.md` verbatim; do not lint-prose them.
#![allow(clippy::doc_markdown)]

pub mod config;
pub mod freshness;
pub mod glob;
pub mod graph;
pub mod lock;
pub mod pubspec;
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
    /// Workspace discovery or artifact validation failed.
    #[error("workspace: {0}")]
    Workspace(String),
    /// Graph inputs disagree (root absent from the config, duplicate
    /// entries, unknown workspace member).
    #[error("package graph: {0}")]
    Graph(String),
}
