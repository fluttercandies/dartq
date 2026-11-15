//! `dartq test`'s substrate: run `dart test` suites faster with output the
//! stock runner would print.
//!
//! - [`cache`] — content-addressed suite result cache: a suite whose
//!   import closure, run-time inputs, resolution, toolchain, arguments and
//!   environment hash to an earlier green run's key replays that run's
//!   reporter events. [`deps`] and [`stamp`] are its substrate (directive
//!   extraction on the dartq scanner, stat-indexed blake3 content stamps),
//!   and [`tokensig`] signs Dart sources by their significant content, so
//!   comment-only and format-only edits re-key nothing.
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

#![allow(clippy::format_push_string, clippy::semicolon_if_nothing_returned)]
#![allow(clippy::too_many_lines)]
#![allow(clippy::needless_pass_by_value)]
#![allow(clippy::type_complexity)]
#![allow(clippy::needless_range_loop)]
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::unnecessary_wraps)]
#![allow(clippy::too_many_arguments)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::assert_is_empty)]
#![allow(clippy::used_underscore_binding)]
#![allow(clippy::manual_let_else)]
#![allow(clippy::default_trait_access)]
#![allow(clippy::ptr_arg)]
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
#![allow(clippy::unnecessary_sort_by)]
#![allow(elided_lifetimes_in_paths)]
#![allow(clippy::suspicious_open_options)]
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
#![allow(clippy::doc_markdown, clippy::cloned_ref_to_slice_refs)]

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
pub mod tokensig;

pub use cache::{Cache, Refusal, RefusalDetail, Runner};
pub use exec::{
    ExecConfig, ExecError, Execution, Loose, Settled, Streamed, SuiteBlock, execute,
    execute_flutter, execute_grouped,
};
pub use group::GroupRunner;
pub use host::{Lease, Profile, cache_dir, lease, lease_processes, lease_run, split};
