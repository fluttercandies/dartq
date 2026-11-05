//! Content-addressed Flutter build cache: cross-worktree sharing of
//! `.dart_tool/flutter_build/<hash>/` artifacts.
//!
//! A PATH shim wraps every `flutter build|run|test`. Before flutter runs,
//! the shim derives a coarse config key for the invocation (toolchain,
//! define vector, target, provenance file contents — no depfile closure,
//! so it is computable on a fresh tree) and looks it up in a
//! machine-local store; on a hit it materializes the cached build
//! directories into the project tree, so flutter_tools' own content
//! validation finds every target up to date and skips the build. After a
//! build that missed, the fresh build directories are ingested back.
//! Source-level divergence the coarse key cannot see is delegated to
//! flutter_tools: a materialized dir whose recorded input hashes do not
//! match the tree rebuilds natively.
//!
//! # Faces
//!
//! * **Contract** ([`types`], [`error`]): content ids ([`BlobId`]), the key
//!   inputs ([`KeyInputs`]), the persisted [`EntryManifest`] with its
//!   per-directory [`types::DirRecord`]s, and the crate-wide
//!   [`BcacheError`].
//! * **Key engine** ([`key`]): the toolchain fingerprint, define vector,
//!   target, and provenance contents, hashed into the cache key
//!   ([`key::config_key`]).
//! * **Store** ([`store`]): the two-layer CAS — entries addressed by key,
//!   payload blobs deduplicated by content — with verified atomic ingest
//!   and refcounted LRU garbage collection. The store root is injected by
//!   callers (default `$XDG_CACHE_HOME/dartq/cas/v1`, falling back to
//!   `~/.cache/dartq/cas/v1`).
//! * **Materializer** ([`materialize`], [`fsutil`]): copy-on-write
//!   placement of an entry's directories into a project tree, rewriting
//!   the project-root paths flutter bakes into its bookkeeping files.
//! * **Flutter surface** ([`args_model`], [`assemble`], [`ingest_scan`]):
//!   the flutter argv model that feeds the key, the `flutter assemble`
//!   argv→template parser, and the post-build scan that turns fresh build
//!   directories into an ingest draft.
//! * **Assemble observer** ([`observer`]): the process-table watch that
//!   reads the nested `flutter assemble` invocations the platform build
//!   systems spawn behind the PATH shim, feeding [`assemble`] their argv.

pub mod args_model;
pub mod assemble;
pub mod error;
pub mod fsutil;
pub mod ingest_scan;
pub mod key;
pub mod materialize;
pub mod observer;
pub mod orchestrator;
pub mod store;
pub mod types;

pub use error::BcacheError;
pub use store::{CasStore, GcReport};
pub use types::{
    BlobId, ClosureFile, DirRecord, EntryManifest, FileIdentity, KeyInputs, ManifestFile, NameRule,
    RewriteKind, StoreStats, TargetSpec, ToolchainFingerprint, blob_id_hex,
};
