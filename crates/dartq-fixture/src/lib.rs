//! dartq-fixture — the behavioral lint-test DSL: markup-annotated Dart
//! fixtures run through the production lint pipeline, plus the converter
//! that ports the upstream linter's rule tests into it mechanically.
//!
//! # Why
//!
//! Upstream lint rules ship behavioral tests as markup bodies
//! (`assertDiagnosticsFromMarkup(r'''…[!…!]…''')`, the `TestCode`
//! grammar) — nothing ported them, so every rule's dartq test suite
//! started from scratch and transcription drift was invisible. This crate
//! is the single fixture format every behavioral case goes through, the
//! way one fixture DSL serves every crate in rust-analyzer.
//!
//! # The two test tools, and which is which
//!
//! - **This DSL** ([`parse`] + [`runner::assert_doc`]) is for behavioral
//!   cases: real Dart source in, emitted lint rows out, (code, UTF-16
//!   range) equality — the upstream `assertDiagnosticsFromMarkup`
//!   contract, plus optional pinned messages.
//! - **Hand-built arena fixtures** (`dartq_lints`' `testsupport`
//!   `Fixture::build`) remain the tool for payload-contract pins — tests
//!   that assert the collapse-map AST *shape* a rule reads. The DSL
//!   cannot express those; the bridge is not a replacement.
//!
//! # Grammar
//!
//! One `.fixture` file: a `//- lint:` header, an optional `//- language:`
//! header pinning the version the cases analyze under, then any number of
//! `//- case:` blocks; each case holds one or more `//- file:` blocks of
//! marked Dart (the first is the analyzed unit), in the upstream inline
//! markup plus optional line-anchored `// ^^^ code` markers — see
//! [`fixture`] for the full grammar. [`port`] and its `port_upstream`
//! binary convert an upstream `*_test.dart` into this format — including
//! the class's `LanguageVersionNNNMixin` pin as the `//- language:` header
//! — rejecting (never silently dropping) anything it cannot represent.
//!
//! # Runner contract
//!
//! [`runner::assert_case`] feeds the analyzed file through the production
//! bridge — scan, parse, arena, registry enable set, engine run — with the
//! version the case resolves to ([`runner::version_of`]: the file's own
//! `// @dart=` override, else the fixture's `//- language:`, else the
//! default) deriving both the parse surface and the rule gates, and the
//! semantic fold mounted exactly when the enabled rule consumes it, the
//! driver's per-file shape. A new rule's behavioral test is one
//! `include_str!` data file and a call to [`runner::assert_doc`] (see
//! `dartq-lints`' `tests/upstream_cases.rs` for the driver shape).

#![allow(clippy::needless_pass_by_value, clippy::semicolon_if_nothing_returned)]
#![allow(clippy::assert_is_empty)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::too_many_lines)]
#![allow(clippy::type_complexity)]
#![allow(clippy::format_push_string)]
#![allow(clippy::manual_let_else)]
#![allow(clippy::default_trait_access)]
#![allow(clippy::ptr_arg)]
#![allow(clippy::match_same_arms)]
#![allow(clippy::used_underscore_binding)]
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
#![allow(clippy::too_many_arguments)]
#![allow(clippy::needless_continue)]
#![allow(clippy::missing_panics_doc)]
#![allow(clippy::trivially_copy_pass_by_ref)]
#![allow(clippy::explicit_counter_loop)]
#![allow(clippy::never_loop)]
#![allow(clippy::needless_range_loop)]
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
#![allow(clippy::cast_possible_wrap)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::cast_precision_loss)]

pub mod fixture;
pub mod markup;
pub mod port;
pub mod runner;

pub use fixture::{
    AnalyzedFile, Case, DEFAULT_FILE, Expectation, FileBlock, FixtureDoc, ParseError, parse,
};
pub use port::{Outcome, PortedCase, merge_divergences, port_file, render};
pub use runner::{assert_case, assert_doc, run_case, version_of};
