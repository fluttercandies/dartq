//! #3139 — the `unnecessary_final` field/super-formal gate: the type
//! annotation, and the declaration-body reach.
//!
//! `_checkFinalParameter` reports the `final` keyword of a
//! `FieldFormalParameter`/`SuperFormalParameter` (upstream Dart SDK
//! source, pkg/analyzer/lib/src/error/best_practices_verifier.dart
//! :1040-1049, called only from `visitFieldFormalParameter`/
//! `visitSuperFormalParameter` :496-504/:902-910). The parser's
//! `fieldFormalParameter` production carries an **optional type
//! annotation** between the keyword and the receiver —
//! `'final' finalConstVarOrType? … 'this'|'super' '.' identifier`
//! (pkg/_fe_analyzer_shared/lib/src/parser/parser_impl.dart,
//! `parseFormalParameter`: the type atom, then the `this`/`super` arm) —
//! so `A(final int this.a)` reports exactly like `A(final this.a)`. The
//! body reach is the other half: only a class/enum/mixin/extension body
//! declares a member whose `(…)` is a formal list; a call inside a
//! statement region (`void f() { C(final this.x); }`) is not a
//! declaration at all. Every pin below mirrors the oracle `3.13.7` row.

use super::test_rig::report_ops_bare;
use super::*;

fn rows_named<'d>(report: &'d FileReport, name: &str) -> Vec<&'d Diagnostic> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == name)
        .collect()
}

/// The byte offset of the `needle`th (0-based) occurrence of `needle`.
fn anchor(source: &str, needle: &str, nth: usize) -> u32 {
    source
        .match_indices(needle)
        .nth(nth)
        .unwrap_or_else(|| panic!("fixture anchor {needle:?}#{nth}"))
        .0 as u32
}

/// `A(final int this.a)` — the typed field formal reports at the `final`
/// keyword (parser_impl.dart `parseFormalParameter` parses the type atom
/// before the `this` arm; the node keeps the `finalKeyword`).
#[test]
fn uf3139_typed_field_formal_fires() {
    let main = "class A {\n  int a;\n  A(final int this.a);\n}\n";
    let report = report_ops_bare(main);
    let rows = rows_named(&report, "unnecessary_final");
    assert_eq!(rows.len(), 1, "{report:?}");
    assert_eq!(rows[0].severity, Severity::Warning);
    assert_eq!(
        rows[0].range,
        SourceRange {
            offset: anchor(main, "final int this", 0),
            length: 5,
        }
    );
}

/// `B(final int super.x)` — the typed super formal reports the same way
/// (upstream `visitSuperFormalParameter` → `_checkFinalParameter`).
#[test]
fn uf3139_typed_super_formal_fires() {
    let main = "class A {\n  A(int x);\n}\nclass B extends A {\n  B(final int super.x) : super(1);\n}\n";
    let report = report_ops_bare(main);
    let rows = rows_named(&report, "unnecessary_final");
    assert_eq!(rows.len(), 1, "{report:?}");
    assert_eq!(
        rows[0].range,
        SourceRange {
            offset: anchor(main, "final int super", 0),
            length: 5,
        }
    );
}

/// Arbitrarily shaped type annotations: type arguments (`Map<String,
/// int>`, the `>>`-closing `List<List<int>>`), a nullable function type
/// (a nested parameter list), a record type (a nested field list), and a
/// bare type parameter. Each reports at its own `final` keyword.
#[test]
fn uf3139_complex_type_annotations_fire() {
    let main = "class A<T> {\n\
                \x20 Map<String, int> m;\n\
                \x20 List<List<int>> l;\n\
                \x20 void Function(int)? f;\n\
                \x20 (int, String) r;\n\
                \x20 T t;\n\
                \x20 A(final Map<String, int> this.m, final List<List<int>> this.l);\n\
                \x20 A.n(final void Function(int)? this.f, final (int, String) this.r, final T this.t);\n\
                }\n";
    let report = report_ops_bare(main);
    let rows = rows_named(&report, "unnecessary_final");
    assert_eq!(rows.len(), 5, "{report:?}");
    let expected = [
        anchor(main, "final Map", 0),
        anchor(main, "final List", 0),
        anchor(main, "final void Function", 0),
        anchor(main, "final (int", 0),
        anchor(main, "final T", 0),
    ];
    let mut actual: Vec<u32> = rows.iter().map(|row| row.range.offset).collect();
    actual.sort_unstable();
    assert_eq!(actual, expected, "{report:?}");
    assert!(rows.iter().all(|row| row.range.length == 5), "{report:?}");
}

/// The optional (`[…]`) and named (`{…}`) formal groups inherit the
/// declaration formal list, so a typed `final` inside them reports too
/// (`C({required final int? this.b})`).
#[test]
fn uf3139_typed_optional_and_named_groups_fire() {
    let main = "class A {\n\
                \x20 int a;\n\
                \x20 int? b;\n\
                \x20 void Function(int) f;\n\
                \x20 A(this.a, [final int this.b = 0]);\n\
                \x20 A.m({required final void Function(int) this.f});\n\
                }\n";
    let report = report_ops_bare(main);
    let rows = rows_named(&report, "unnecessary_final");
    assert_eq!(rows.len(), 2, "{report:?}");
    let expected = [
        anchor(main, "final int this.b", 0),
        anchor(main, "final void Function", 0),
    ];
    let mut actual: Vec<u32> = rows.iter().map(|row| row.range.offset).collect();
    actual.sort_unstable();
    assert_eq!(actual, expected, "{report:?}");
}

/// A statement region is not a declaration body: the `C(final this.x)` of
/// a function or getter BODY is a call, never a formal list — upstream
/// never reaches `visitFieldFormalParameter` on it (the call's arguments
/// are expressions), so no WARNING is reported there.
#[test]
fn uf3139_statement_body_call_silent() {
    let main = "class C {\n\
                \x20 int x;\n\
                \x20 C(final int this.x);\n\
                \x20 int get g {\n\
                \x20   C(final this.x);\n\
                \x20   return 1;\n\
                \x20 }\n\
                }\n\
                void f() {\n\
                \x20 C(final this.x);\n\
                }\n";
    let report = report_ops_bare(main);
    let rows = rows_named(&report, "unnecessary_final");
    assert_eq!(rows.len(), 1, "only the constructor's own formal: {report:?}");
    assert_eq!(
        rows[0].range,
        SourceRange {
            offset: anchor(main, "final int this", 0),
            length: 5,
        }
    );
}

/// A `class` inside a statement region is not a declaration at all — Dart
/// has no local classes, so the parser reads that `class` as an identifier
/// and recovers; the oracle reports no `unnecessary_final` for the
/// pseudo-constructor's `final this.x`.
#[test]
fn uf3139_local_class_pseudo_ctor_silent() {
    let main = "void f() {\n\
                \x20 class C {\n\
                \x20   C(final this.x);\n\
                \x20   int x;\n\
                \x20 }\n\
                }\n";
    let report = report_ops_bare(main);
    assert!(
        rows_named(&report, "unnecessary_final").is_empty(),
        "{report:?}"
    );
}

/// The primary-constructor suppress covers the typed spelling too: a
/// declaring parameter's `final` is the declaration, not a redundant
/// modifier (`_inPrimaryConstructorDeclaration`,
/// best_practices_verifier.dart:101/:818-824).
#[test]
fn uf3139_typed_primary_ctor_suppressed() {
    for main in [
        "class Q(final int this.x) {\n  int x;\n}\n",
        "class Q<T>(final T this.x) {\n  T x;\n}\n",
        "class Q.n(final int this.x) {\n  int x;\n}\n",
    ] {
        let report = report_ops_bare(main);
        assert!(
            rows_named(&report, "unnecessary_final").is_empty(),
            "{main:?}: {report:?}"
        );
    }
}

/// A constructor's initializer list and a `const` constructor keep the
/// declaration formal list reach (`)`-follow `:` / `;`), so the typed
/// `final` still reports.
#[test]
fn uf3139_typed_formal_with_initializer_fires() {
    let main = "class A {\n\
                \x20 int a;\n\
                \x20 A(final int this.a) : super();\n\
                }\n\
                class B {\n\
                \x20 int b;\n\
                \x20 const B(final int this.b);\n\
                }\n";
    let report = report_ops_bare(main);
    let rows = rows_named(&report, "unnecessary_final");
    assert_eq!(rows.len(), 2, "{report:?}");
    let expected = [anchor(main, "final int this.a", 0), anchor(main, "final int this.b", 0)];
    let mut actual: Vec<u32> = rows.iter().map(|row| row.range.offset).collect();
    actual.sort_unstable();
    assert_eq!(actual, expected, "{report:?}");
}
