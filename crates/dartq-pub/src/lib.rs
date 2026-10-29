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
