//! Content-addressed Flutter build cache: cross-worktree sharing of
//! `.dart_tool/flutter_build/<hash>/` artifacts, plus the `flutter test`
//! kernel cache (`build/test_cache`, same-tree wipe recovery).
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
//! flutter_tools, and that delegation is only sound because of what the
//! replay rules guarantee about the recorded paths: paths under the
//! ingest worktree root are rewritten to the target tree, paths under
//! the pub cache or the Flutter SDK root stay verbatim (machine-local
//! constants), and a build dir that mentions any other absolute path is
//! refused cross-tree ([`types::PathAnchors`]); on the test face a
//! foreign-URI dill is placed verbatim rather than rewritten. A
//! materialized dir whose recorded input hashes then mismatch the tree
//! rebuilds natively — and, on the test face, a stale kernel-cache dill
//! is invalidated by frontend_server itself. A hit never replaces an
//! existing build dir; it only fills missing files, after verifying
//! every referenced blob exists.
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
//!   callers (default `$XDG_CACHE_HOME/dartq/cas/{STORE_LAYOUT_VERSION}`,
//!   falling back to `~/.cache/dartq/cas/{STORE_LAYOUT_VERSION}`).
//! * **Materializer** ([`materialize`], [`fsutil`], [`dill_rewrite`]):
//!   copy-on-write placement of an entry's directories into a project
//!   tree, filling only missing files and rewriting the worktree-root
//!   paths — and the `flutter_build/<hash>` dir-name segments — flutter
//!   bakes into its bookkeeping files; on the test face's kernel-cache
//!   dills replayed into a different tree, the `file://` URI prefixes
//!   baked into the kernel tables are rewritten too ([`dill_rewrite`]).
//! * **Flutter surface** ([`args_model`], [`assemble`], [`ingest_scan`]):
//!   the flutter argv model that feeds the key, the `flutter assemble`
//!   argv→template parser, and the post-build scan that turns fresh build
//!   directories — and, for `flutter test`, the fixed-path kernel-cache
//!   dir — into an ingest draft.
//! * **Assemble observer** ([`observer`]): the process-table watch that
//!   reads the nested `flutter assemble` invocations the platform build
//!   systems spawn behind the PATH shim, feeding [`assemble`] their argv.

pub mod args_model;
pub mod assemble;
pub mod dill_rewrite;
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
pub use store::{CasStore, GcReport, STORE_LAYOUT_VERSION};
pub use types::{
    BlobId, ClosureFile, DirRecord, EntryManifest, FileIdentity, FileStamp, KeyInputs, ManifestFile,
    NameRule, PathAnchors, PlacedFile, RewriteKind, StoreStats, TargetSpec, ToolchainFingerprint,
    blob_id_hex,
};
