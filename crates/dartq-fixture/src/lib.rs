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
//! One `.fixture` file: a `//- lint:` header, then any number of
//! `//- case:` blocks; each case holds one or more `//- file:` blocks of
//! marked Dart (the first is the analyzed unit), in the upstream inline
//! markup plus optional line-anchored `// ^^^ code` markers — see
//! [`fixture`] for the full grammar. [`port`] and its `port_upstream`
//! binary convert an upstream `*_test.dart` into this format, rejecting
//! (never silently dropping) anything it cannot represent.
//!
//! # Runner contract
//!
//! [`runner::assert_case`] feeds the analyzed file through the production
//! bridge — scan, parse, arena, registry enable set, engine run — with
//! the semantic fold mounted exactly when the enabled rule consumes it,
//! the driver's per-file shape. A new rule's behavioral test is one
//! `include_str!` data file and a call to [`runner::assert_doc`] (see
//! `dartq-lints`' `tests/upstream_cases.rs` for the driver shape).

pub mod fixture;
pub mod markup;
pub mod port;
pub mod runner;

pub use fixture::{
    AnalyzedFile, Case, DEFAULT_FILE, Expectation, FileBlock, FixtureDoc, ParseError, parse,
};
pub use port::{Outcome, PortedCase, merge_divergences, port_file, render};
pub use runner::{assert_case, assert_doc, run_case};
