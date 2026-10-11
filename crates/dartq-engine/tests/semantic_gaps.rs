//! The #304 semantic-gap contracts at the engine boundary: each gap the
//! semantic-lane audit spun out of the 32-red adjudication, pinned
//! end-to-end through [`Engine::analyze`] with the default diagnoser.
//!
//! The diagnosing code lives behind the engine's [`crate::semdiag`] seam,
//! in `dartq-semantic` (`diagnose.rs`) — this crate only feeds the faces
//! (graph, namespaces, residency-wide element index). A same-file shape
//! runs the identical diagnose code and faces on either side of the seam,
//! so each pin's hand-trace matches the diagnoser's own unit suite.
//!
//! * E1 — an enum constant's constructor type-argument group must check
//!   against the enclosing enum's declared type-parameter count
//!   (`wrong_number_of_type_arguments_enum`); the diagnoser's
//!   `in_enum_element` snapshot latches at the `EnumValueDeclaration`
//!   identifier and answers only the group at the constant's own
//!   position — a group inside the constructor arguments (an open
//!   argument list) checks its own head instead (the precision the
//!   `arg_levels` guard pins, diagnoser-side under
//!   `enum_constant_argument_groups_check_their_own_heads`).
//! * E2 — a `null` literal argument into a known non-nullable formal must
//!   report `argument_type_not_assignable`; `check_arguments` reconciles
//!   the closed argument list against the callee's declared formals
//!   (positional by index, named by spelling) and decides on nullability
//!   alone.
//! * E3 — the import face the diagnoser resolves through is the engine's
//!   [`Namespaces`] build (`crates/dartq-engine/src/pipeline.rs:56`): the
//!   merged import namespace with build-time combinator filtering and the
//!   per-prefix tables (`Namespaces::lookup` / `lookup_prefixed`, consulted
//!   by `lookup_scoped` and `prefixed_hit`). An unfiltered import's face
//!   carries top-level functions, so a bare call resolves; a prefixed
//!   import stays out of the bare face; `show`/`hide` filter it. The
//!   original red here was not a face gap: the pins wrote the imported
//!   file to disk only, so the import target mapped to no graph library
//!   (edgeless — the `uri_does_not_exist` symptom) and no name could
//!   resolve. The pins feed both files through [`Engine::set_content`]
//!   (the established multi-file pattern, `semdiag.rs`'s
//!   `analyze_two_libraries`). `LibraryScopes` (`library.rs`) stays the
//!   query-surface face — not a resolution prerequisite.

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
/// group). Diagnoser-side twin: `enum_constant_type_args_report_enum_code`.
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

/// E1 precision (negative): a generic invocation inside the constant's
/// constructor arguments checks its own head — the enum's declared count
/// must not leak into it (`h<A>` matches `h`'s declared count; the
/// constant's own group is absent). Diagnoser-side twin:
/// `enum_constant_argument_groups_check_their_own_heads`.
#[test]
fn enum_constant_argument_groups_check_their_own_heads_end_to_end() {
    const SOURCE: &str = "class A {}\nvoid h<T>() {}\nenum E<T, U> { a(h<A>()); }\n";
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
        "the snapshot must not leak into the arguments' groups: {names:?}"
    );
}

/// E1 selector phrase (positive): the constant's type arguments attach to
/// the HEAD identifier (`a<D>.other()` — `parseEnumElement` parses one
/// group at the head, parser_impl.dart:2904-2947; group after the
/// selector is the `constructorWithTypeArguments` recovery error,
/// parser_impl.dart:8884-8899) — 2 declared, 1 given reports the `_enum`
/// code at the group.
#[test]
fn enum_constant_selector_type_args_report_enum_code_end_to_end() {
    const SOURCE: &str = "class D {}\nenum E<T, U> { a<D>.other(); const E.other(); }\n";
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
        "the selector's group is the constant's group: {names:?}"
    );
}

/// E2 positive: the `null` literal into a known non-nullable positional
/// formal is decidable from nullability alone (`argument_type_not_
/// assignable`). Diagnoser-side twin:
/// `null_argument_reports_argument_type_not_assignable`.
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

/// E2 named shape (positive): the named argument reconciles by spelling —
/// `g(p: null)` into a non-nullable named formal reports.
#[test]
fn null_named_argument_reports_argument_type_not_assignable_end_to_end() {
    const SOURCE: &str = "void g({int p = 0}) {}\nvoid main() { g(p: null); }\n";
    let fixture = Fixture::new(&[]);
    let mut engine = fixture.engine();
    let main = engine.set_content(&fixture.path("lib/main.dart"), SOURCE.as_bytes());
    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("analyze completes");
    assert_eq!(
        codes(&diagnostics, main),
        ["argument_type_not_assignable"],
        "null into a non-nullable named formal must report"
    );
}

/// E3 positive: an unfiltered import's face carries top-level
/// functions, so the bare call resolves through the engine's graph and
/// namespace build — zero diagnostics for the pair. Both files feed the
/// analyzed world (`set_content`): the import face is the graph
/// namespaces' merge.
#[test]
fn imported_top_level_fn_resolves_through_unfiltered_import() {
    let fixture = Fixture::new(&[("lib/other.dart", "int fa() => 1;\n")]);
    let mut engine = fixture.engine();
    engine.set_content(&fixture.path("lib/other.dart"), b"int fa() => 1;\n");
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
#[test]
fn prefixed_import_stays_out_of_the_bare_face() {
    let fixture = Fixture::new(&[("lib/other.dart", "int fa() => 1;\n")]);
    let mut engine = fixture.engine();
    engine.set_content(&fixture.path("lib/other.dart"), b"int fa() => 1;\n");
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

/// E3 combinator face (positive): `show fa` filters the import namespace
/// at build time, and the shown name resolves bare.
#[test]
fn shown_import_name_resolves_bare() {
    let fixture = Fixture::new(&[("lib/other.dart", "int fa() => 1;\nint fb() => 2;\n")]);
    let mut engine = fixture.engine();
    engine.set_content(
        &fixture.path("lib/other.dart"),
        b"int fa() => 1;\nint fb() => 2;\n",
    );
    let main = engine.set_content(
        &fixture.path("lib/main.dart"),
        b"import 'other.dart' show fa;\nvoid main() { fa(); }\n",
    );
    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("analyze completes");
    let names = codes(&diagnostics, main);
    assert!(
        names.is_empty(),
        "the shown name must resolve through the filtered face: {names:?}"
    );
}

/// E3 combinator face (negative): `hide fa` removes the name from the
/// import namespace at build time — the bare call reports.
#[test]
fn hidden_import_name_reports_bare() {
    let fixture = Fixture::new(&[("lib/other.dart", "int fa() => 1;\nint fb() => 2;\n")]);
    let mut engine = fixture.engine();
    engine.set_content(
        &fixture.path("lib/other.dart"),
        b"int fa() => 1;\nint fb() => 2;\n",
    );
    let main = engine.set_content(
        &fixture.path("lib/main.dart"),
        b"import 'other.dart' hide fa;\nvoid main() { fa(); }\n",
    );
    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("analyze completes");
    assert_eq!(
        codes(&diagnostics, main),
        ["undefined_function"],
        "the hidden name must stay out of the bare face"
    );
}
