//! The #304 semantic-gap contracts at the engine boundary: each gap the
//! semantic-lane audit spun out of the 32-red adjudication, pinned
//! end-to-end through [`Engine::analyze`] with the default diagnoser.
//!
//! The fixes live behind the engine's [`crate::semdiag`] seam, in
//! `dartq-semantic` (`diagnose.rs` / `library.rs`) — this crate only feeds
//! the faces (graph, namespaces, residency-wide element index). Every pin
//! here is `#[ignore]`d until its fix lands: a same-file shape runs the
//! identical diagnose code and faces on either side of the seam, so the
//! red here matches the diagnoser's own unit suite, and each pin flips
//! green with its fix, no edits. E3's engine-side half is red for the same
//! reason: the engine face seeding goes through `Namespaces::build`
//! (`crates/dartq-engine/src/pipeline.rs:56` → `lookup_scoped`) and never
//! consults `LibraryScopes::build` (`crates/dartq-semantic/src/library.rs:
//! 139-173`), so imported top-level names do not resolve end-to-end.
//!
//! * E1 — an enum constant's constructor type-argument group must check
//!   against the enclosing enum's declared type-parameter count
//!   (`wrong_number_of_type_arguments_enum`); the diagnoser's
//!   `in_enum_element` snapshot must be latched at the
//!   `EnumValueDeclaration` identifier and consumed by
//!   `end_type_arguments` before the constructor-reference shape.
//! * E2 — a `null` literal argument into a known non-nullable formal must
//!   report `argument_type_not_assignable`; the positional actuals must
//!   reconcile against the callee's declared formals at
//!   `close_arguments`/`consume_send`.
//! * E3 — an unfiltered import's face carries top-level functions (the
//!   library namespace merge), so a bare call resolves; a prefixed import
//!   stays out of the bare face. Exercised through the real engine graph,
//!   which the diagnoser's unit harness cannot assemble. Red until
//!   `dartq-semantic` wires `LibraryScopes` into the engine face (the
//!   `pipeline.rs:56` / `library.rs:139-173` seam above).

mod common;

use common::Fixture;
use dartq_engine::CancellationToken;
use dartq_source::FileId;

/// The code names of one file's diagnostics, sorted (order-insensitive
/// family pins; the canonical-order contract is `pipeline.rs`'s).
fn codes(diagnostics: &[dartq_diagnostics::Diagnostic], file: FileId) -> Vec<&'static str> {
    let mut names: Vec<&'static str> = diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.file == file)
        .map(|diagnostic| diagnostic.code.name)
        .collect();
    names.sort();
    names
}

/// E1 positive: the constant's type-argument group checks against the
/// enum's declared count (2 declared, 1 given → the `_enum` code at the
/// group). Diagnoser-side gap: the `in_enum_element` snapshot must be
/// latched when the constant's identifier fires and must win over the
/// constructor-reference shape in `end_type_arguments`.
#[ignore = "E1 (#304): under-reports until dartq-semantic latches the enum-constant type-argument snapshot; flip when the fix lands"]
#[test]
fn enum_constant_type_args_report_enum_code_end_to_end() {
    const SOURCE: &str = "class D {}\nenum E<T, U> { a<D>(); }\n";
    let fixture = Fixture::new(&[]);
    let mut engine = fixture.engine();
    let main = engine.set_content(&fixture.path("lib/main.dart"), SOURCE.as_bytes());
    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("analyze completes");
    let names = codes(&diagnostics, main);
    assert!(
        names
            .iter()
            .any(|&name| name == "wrong_number_of_type_arguments_enum"),
        "the `_enum` code must fire at the constant's group: {names:?}"
    );
}

/// E1 boundary: a group matching the declared count (and a group-less
/// constant) stays silent in the family — the check fires only on the
/// arity mismatch, never on the shape.
#[ignore = "E1 (#304): runs with the positive pin once dartq-semantic latches the snapshot"]
#[test]
fn enum_constant_matching_type_args_stay_silent() {
    const SOURCE: &str = "enum E<T> { a<int>(); }\nenum F<T> { b; }\n";
    let fixture = Fixture::new(&[]);
    let mut engine = fixture.engine();
    let main = engine.set_content(&fixture.path("lib/main.dart"), SOURCE.as_bytes());
    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("analyze completes");
    let names = codes(&diagnostics, main);
    assert!(
        !names
            .iter()
            .any(|&name| name.starts_with("wrong_number_of_type_arguments")),
        "matching arity and no group must stay silent: {names:?}"
    );
}

/// E2 positive: the `null` literal into a known non-nullable positional
/// formal is decidable from nullability alone (`argument_type_not_
/// assignable`). Diagnoser-side gap: the closed argument list must
/// reconcile against the callee's declared formals.
#[ignore = "E2 (#304): under-reports until dartq-semantic reconciles positional actuals with callee formals; flip when the fix lands"]
#[test]
fn null_argument_reports_argument_type_not_assignable_end_to_end() {
    const SOURCE: &str = "void g(int p) {}\nvoid main() { g(null); }\n";
    let fixture = Fixture::new(&[]);
    let mut engine = fixture.engine();
    let main = engine.set_content(&fixture.path("lib/main.dart"), SOURCE.as_bytes());
    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("analyze completes");
    let names = codes(&diagnostics, main);
    assert!(
        names
            .iter()
            .any(|&name| name == "argument_type_not_assignable"),
        "null into a non-nullable formal must report: {names:?}"
    );
}

/// E2 boundary: the nullability decision cuts both ways — `null` into a
/// nullable formal is assignable, a non-null literal into a non-nullable
/// formal is assignable; the family stays silent on both.
#[ignore = "E2 (#304): runs with the positive pin once the reconciliation lands"]
#[test]
fn null_argument_boundaries_stay_silent() {
    const SOURCE: &str =
        "void soft(int? p) {}\nvoid hard(int p) {}\nvoid main() { soft(null); hard(1); }\n";
    let fixture = Fixture::new(&[]);
    let mut engine = fixture.engine();
    let main = engine.set_content(&fixture.path("lib/main.dart"), SOURCE.as_bytes());
    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("analyze completes");
    let names = codes(&diagnostics, main);
    assert!(
        !names
            .iter()
            .any(|&name| name == "argument_type_not_assignable"),
        "assignable arguments must stay silent: {names:?}"
    );
}

/// E3 positive: an unfiltered import's face carries top-level
/// functions, so the bare call resolves through the engine's graph and
/// namespace build — zero diagnostics for the pair. This is the
/// engine-side half of the E3 contract: the faces the diagnoser consumes
/// are correct whatever the face-level fix in `dartq-semantic` does.
#[ignore = "E3 (#304): under-reports until dartq-semantic wires LibraryScopes into the engine face — seeding is Namespaces::build only (crates/dartq-engine/src/pipeline.rs:56), never LibraryScopes::build (crates/dartq-semantic/src/library.rs:139-173); flip when that wiring lands"]
#[test]
fn imported_top_level_fn_resolves_through_unfiltered_import() {
    let fixture = Fixture::new(&[("lib/other.dart", "int fa() => 1;\n")]);
    let mut engine = fixture.engine();
    let main = engine.set_content(
        &fixture.path("lib/main.dart"),
        b"import 'other.dart';\nvoid main() { fa(); }\n",
    );
    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("analyze completes");
    let names = codes(&diagnostics, main);
    assert!(
        names.is_empty(),
        "the imported top-level function must resolve: {names:?}"
    );
}

/// E3 boundary: a prefixed import keeps its names out of the bare
/// face — the prefix table is separate (`_prefixElements`), so the bare
/// call reports while `p.fa()` would resolve. Pins the merge boundary the
/// face seeding must preserve.
#[ignore = "E3 (#304): the bare face never consults LibraryScopes::build (crates/dartq-semantic/src/library.rs:139-173); seeding goes through Namespaces::build only (crates/dartq-engine/src/pipeline.rs:56) — flip when dartq-semantic wires LibraryScopes into the engine face"]
#[test]
fn prefixed_import_stays_out_of_the_bare_face() {
    let fixture = Fixture::new(&[("lib/other.dart", "int fa() => 1;\n")]);
    let mut engine = fixture.engine();
    let main = engine.set_content(
        &fixture.path("lib/main.dart"),
        b"import 'other.dart' as p;\nvoid main() { fa(); }\n",
    );
    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("analyze completes");
    assert_eq!(
        codes(&diagnostics, main),
        ["undefined_function"],
        "the prefixed import contributes no bare names"
    );
}
