//! Content-addressed Flutter build cache: cross-worktree sharing of
//! `.dart_tool/flutter_build/<hash>/` artifacts, plus the `flutter test`
//! kernel cache (`build/test_cache`, same-tree wipe recovery).
//!
//! A PATH shim wraps every `flutter build|run|test`. Before flutter runs,
//! the shim derives a coarse config key for the invocation (toolchain,
//! define vector, target, provenance file contents — no depfile closure,
//! so it is computable on a fresh tree) and looks it up in a
//! machine-local store; on a hit it materializes the cached build
//! directories into the project tree, so `flutter_tools`' own content
//! validation finds every target up to date and skips the build. After a
//! build that missed, the fresh build directories are ingested back.
//! Source-level divergence the coarse key cannot see is delegated to
//! `flutter_tools`, and that delegation is only sound because of what the
//! replay rules guarantee about the recorded paths: paths under the
//! ingest worktree root are rewritten to the target tree, paths under
//! the pub cache or the Flutter SDK root stay verbatim (machine-local
//! constants), and a build dir that mentions any other absolute path is
//! refused cross-tree ([`types::PathAnchors`]); on the test face a
//! foreign-URI dill is placed verbatim rather than rewritten. A
//! materialized dir whose recorded input hashes then mismatch the tree
//! rebuilds natively — and, on the test face, a stale kernel-cache dill
//! is invalidated by `frontend_server` itself. A hit never replaces an
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
//!   callers (default `$XDG_CACHE_HOME/dartq/cas`, falling back to
//!   `~/.cache/dartq/cas`).
//! * **Materializer** ([`materialize`], [`fsutil`], [`dill_rewrite`]):
//!   copy-on-write placement of an entry's directories into a project
//!   tree, filling only missing files and rewriting the worktree-root
//!   paths — and the `flutter_build/<hash>` dir-name segments — flutter
//!   bakes into its bookkeeping files; on the test face's kernel-cache
//!   dills replayed into a different tree, the `file://` URI prefixes
//!   baked into the kernel tables are rewritten too ([`dill_rewrite`]),
//!   and a placed `.filecache` records the md5 of every file the replay
//!   rewrote, so flutter's content validation accepts them. Each tree
//!   keeps a per-entry record of what was placed under which identity
//!   ([`placement`]), so later runs in that tree re-reference the
//!   placed files without reading them while the shared entry stays as
//!   ingested.
//! * **Flutter surface** ([`args_model`], [`assemble`], [`ingest_scan`]):
//!   the flutter argv model that feeds the key, the `flutter assemble`
//!   argv→template parser, and the post-build scan that turns fresh build
//!   directories — and the fixed-path dirs: the `build bundle` asset
//!   dir, the `flutter test` kernel cache — into an ingest draft.
//! * **Assemble observer** ([`observer`]): the process-table watch that
//!   reads the nested `flutter assemble` invocations the platform build
//!   systems spawn behind the PATH shim, feeding [`assemble`] their argv.
//! * **Orphan reaper** ([`reaper`], unix): the one-`ps` sweep after each
//!   modeled run that SIGTERMs the parentless `frontend_server` process
//!   `flutter_tools` leaves behind.

#![allow(clippy::too_many_lines, clippy::semicolon_if_nothing_returned)]
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::unnecessary_wraps)]
#![allow(clippy::too_many_arguments)]
#![allow(clippy::type_complexity)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::assert_is_empty)]
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
    clippy::many_single_char_names,
    clippy::same_item_push
)]

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
pub mod placement;
#[cfg(unix)]
pub mod reaper;
pub mod store;
pub mod types;

pub use error::BcacheError;
pub use store::{CasStore, GcReport};
pub use types::{
    BlobId, ClosureFile, DirRecord, EntryManifest, FileIdentity, FileStamp, KeyInputs,
    ManifestFile, NameRule, PathAnchors, PlacedFile, RewriteKind, StoreStats, TargetSpec,
    ToolchainFingerprint, blob_id_hex,
};
