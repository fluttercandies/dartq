//! Agent-facing logic: query, impact, explain, compatibility, and fix.
//!
//! Landed faces:
//!
//! * [`compatibility`] — the compatibility query: the same base
//!   code under several SDK contexts, aggregated into a per-axis ✓/✗
//!   matrix with the first failing diagnostic per axis.
//! * [`explain`] — the explain query: the one-stop symbol
//!   report (declaration, supertypes, members, references, the declaring
//!   library's importers, and its tests) over borrowed index faces.
//! * [`diagnostic_explain`] — the `explain --diagnostic` report: a single
//!   diagnostic's echo, registry facts, fix rows, same-code neighbours,
//!   and docs URL over borrowed faces.
//! * [`impact`] — the impact query: the changed symbols with
//!   their signature verdicts, the reverse-dependency closure tagged
//!   content-only vs semantic, the test-graph join, and the environment
//!   matrix (v1: one always-compatible context).
//! * [`rev_import_graph`] — the reversed import graph as one shared CSR
//!   build (`impact`'s inversion, reused by the amortized batch queries
//!   and the T8 explain stage).

#![allow(clippy::too_many_lines, clippy::semicolon_if_nothing_returned)]
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
#![allow(clippy::unnecessary_wraps)]
#![allow(clippy::unused_async)]
#![allow(clippy::used_underscore_items)]
#![allow(clippy::struct_excessive_bools)]
#![allow(clippy::struct_field_names)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::redundant_pub_crate)]
#![allow(clippy::must_use_candidate)]
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::doc_markdown)]

pub mod compatibility;
pub mod diagnostic_explain;
pub mod explain;
pub mod impact;
pub mod rev_import_graph;
