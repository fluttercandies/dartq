//! Ticket #3738 pins: the unused-family faces recovered against the
//! 3.13.7 oracle. Each pin fixes one observable divergence the census
//! reported — a wildcard `_` member that must report, a `_` mixin
//! application that must not, an enum-constant receiver chain, a
//! `Function`-head call, a literal index compound write, a `catch (late
//! e)` keyword, a used-override spare, a duplicate enum constructor at
//! its last site, and the unresolved-URI `''` arm of `unused_import`.
//!
//! Every fixture's expected shape was verified against
//! `~/.dartq-oracle/3.13.7/dart-sdk/bin/dart analyze` before the fix
//! landed; the shapes were verified net-neutral against the lib suite.

use super::test_rig::{CORE_PRELUDE, RigImport, report_ops, report_ops_imports, report_ops_wildcards};
use super::*;

/// The rows of `code` as the exact anchored source text, source order.
fn spans_of(report: &FileReport, full: &str, code: &str) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| {
            full[d.range.offset as usize..(d.range.offset + d.range.length) as usize].to_string()
        })
        .collect()
}

// -- Wildcard `_` member -----------------------------------------------
//
// A `_` member still reports `unused_element` when the overridden
// element it hides is never used: upstream `_isReadMember`'s
// `_overridesUsedElement` tail (unused_local_elements_verifier.dart
// :1016) requires the OVERRIDDEN element to be read, not merely to
// exist. An unused super `_` member does not spare the subclass's
// shadowing `_` member.

/// `C1 extends ContainsWildcardVariable` declaring `int get _` — the
/// super's `_` is never read, so the subclass's `_` reports (the
/// `overrides_used_element` walk finds the overridden `_` exists but is
/// not used).
#[test]
fn wildcard_member_reports_when_super_unused() {
    let fixture = "class ContainsWildcardVariable {\n\
                   \x20 int? _;\n\
                   }\n\
                   class C1 extends ContainsWildcardVariable {\n\
                   \x20 int? get _ => 0;\n\
                   }\n\
                   void f() { C1(); }\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops_wildcards(fixture, true);
    assert!(
        spans_of(&report, &full, "unused_element")
            .iter()
            .any(|s| s == "_"),
        "C1._ reports: overridden _ exists but is never read"
    );
}

/// The dual: when the overridden `_` IS read (through the super type),
/// the subclass's `_` stays silent — the recursive walk reaches the
/// used element.
#[test]
fn wildcard_member_silent_when_super_read() {
    let fixture = "class ContainsWildcardVariable {\n\
                   \x20 int? _;\n\
                   }\n\
                   class C2 extends ContainsWildcardVariable {\n\
                   \x20 int? get _ => 0;\n\
                   }\n\
                   void f() {\n\
                   \x20 var c = ContainsWildcardVariable();\n\
                   \x20 c._;\n\
                   }\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops_wildcards(fixture, true);
    assert!(
        !spans_of(&report, &full, "unused_element")
            .iter()
            .any(|s| s == "_"),
        "C2._ silent: the super's _ IS read, overridesUsedElement spares it"
    );
}

/// `class _ = S with M` — a wildcard-named mixin application is spared:
/// the name `_` under `wildcard_variables` is unreferencable, but the
/// declaration still applies a mixin and upstream keeps it out of the
/// unused-`element` set. A `class _ {}` plain declaration reports.
#[test]
fn wildcard_class_mixin_application_spared() {
    let fixture = "class S {}\n\
                   mixin M {}\n\
                   class _ = S with M;\n\
                   void f() { S(); }\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops_wildcards(fixture, true);
    assert!(
        spans_of(&report, &full, "unused_element").is_empty(),
        "class _ = S with M spares: mixin application still applies the mixin"
    );
}

/// `class _ {}` reports — the same wildcard name without the mixin
/// application arm's spare.
#[test]
fn wildcard_class_plain_reports() {
    let fixture = "class _ {}\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops_wildcards(fixture, true);
    assert!(
        spans_of(&report, &full, "unused_element")
            .iter()
            .any(|s| s == "_"),
        "class _ reports: no mixin-application spare"
    );
}

// -- Enum-constant receiver chain --------------------------------------
//
// `E6.e1._()` — the receiver's declared type is the enum `E6`, the
// member `_` reads through a `Receiver::Unknown` chain. The
// `chained_receiver_ty` walk resolves the enum constant's face and the
// `chained_member_credits`/`shorthand_member_refs` pools union the
// member into `member_used_elements` and `member_read_owners`.

/// `enum E6 { e1, e2 }` with `int get _`, then `E6.e1._()` — the `_`
/// member is read through the enum constant receiver and must not
/// report.
#[test]
fn enum_constant_receiver_member_read() {
    let fixture = "enum E6 {\n\
                   \x20 e1,\n\
                   \x20 e2;\n\
                   \x20 const E6();\n\
                   \x20 int get _ => 0;\n\
                   }\n\
                   void f() { E6.e1._(); }\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops_wildcards(fixture, true);
    assert!(
        !spans_of(&report, &full, "unused_element")
            .iter()
            .any(|s| s == "_"),
        "E6._ silent: enum-constant receiver reads the member"
    );
}

// -- Method-tear-off on a `Function` head -------------------------------
//
// `foo<int>.m` — the `foo<int>` instantiation yields `Function`, and
// `.m` reads the extension member `m` on `Function`. The `fn_heads`
// table now carries `DeclKind::Member{Method}` so the member's
// declarer set answers `m` instead of falling through.

/// `extension E on Function { void m() {} }` and `foo<int>.m` — the
/// extension member `m` is read through the `Function` receiver.
#[test]
fn function_head_extension_member_read() {
    let fixture = "extension E on Function {\n\
                   \x20 void m() {}\n\
                   }\n\
                   void foo<T>() {}\n\
                   void f() { foo<int>.m; }\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert!(
        !spans_of(&report, &full, "unused_element")
            .iter()
            .any(|s| s == "m"),
        "extension m silent: Function-head tear-off credits it"
    );
}

// -- Index compound assignment on an unnamed extension -------------------
//
// Upstream `visitIndexExpression` credits `node.writeOrReadElement`
// (unused_local_elements_verifier.dart:196-200): `x[i] ??= v` and
// `x[i] += v` record the `[]=` write element alone, so `[]` keeps its
// own verdict — and a member of an UNNAMED extension is a real
// `unused_element` candidate (`_isPubliclyAccessible` answers
// `enclosingElement.isPublic`, false for a nameless face,
// verifier.dart:870-873). The oracle on
// `extension<R, W> on Indexable<R, W> { operator []; operator []= }`
// with `x[0] ??= 1` reports exactly `The declaration '[]' isn't
// referenced.` at the `[]` span.

/// `x[0] ??= 1` credits only `[]=`: `[]` reports `unused_element`,
/// `[]=` stays silent (the if_null_assignment_implicit_extension corpus
/// shape, verified against the oracle).
#[test]
fn index_compound_assignment_credits_only_index_set() {
    let fixture = "class Indexable<R, W> {\n\
                   \x20 final R _value;\n\
                   \x20 Indexable(this._value);\n\
                   }\n\
                   extension<R, W> on Indexable<R, W> {\n\
                   \x20 R operator [](int index) => _value;\n\
                   \x20 void operator []=(int index, W value) {}\n\
                   }\n\
                   void f() {\n\
                   \x20 var x = Indexable<String, int>('');\n\
                   \x20 x[0] ??= 1;\n\
                   }\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    let unused: Vec<_> = spans_of(&report, &full, "unused_element");
    assert!(
        unused.iter().any(|s| s == "[]"),
        "[] reports: ??= credits the write element []= alone"
    );
    assert!(
        !unused.iter().any(|s| s == "[]="),
        "[]= silent: the compound assignment's writeOrReadElement is []="
    );
}

/// The `+=` tail behaves identically — `[]` reports, `[]=` silent.
#[test]
fn index_plus_assignment_credits_only_index_set() {
    let fixture = "class Indexable<R, W> {\n\
                   \x20 final R _value;\n\
                   \x20 Indexable(this._value);\n\
                   }\n\
                   extension<R, W> on Indexable<R, W> {\n\
                   \x20 R operator [](int index) => _value;\n\
                   \x20 void operator []=(int index, W value) {}\n\
                   }\n\
                   void f() {\n\
                   \x20 var x = Indexable<String, int>('');\n\
                   \x20 x[0] += 1;\n\
                   }\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    let unused: Vec<_> = spans_of(&report, &full, "unused_element");
    assert!(
        unused.iter().any(|s| s == "[]"),
        "[] reports: += credits the write element []= alone"
    );
    assert!(
        !unused.iter().any(|s| s == "[]="),
        "[]= silent: the compound assignment's writeOrReadElement is []="
    );
}

/// The null-aware `x?[i] ??= v` face — the census's exact shape
/// (`maybeIndexable(...)?[0] ??= …` in the
/// if_null_assignment_implicit_extension corpus) — follows the same
/// write-element credit: `[]` reports, `[]=` silent.
#[test]
fn null_aware_index_compound_credits_only_index_set() {
    let fixture = "class Indexable<R, W> {\n\
                   \x20 final R _value;\n\
                   \x20 Indexable(this._value);\n\
                   }\n\
                   extension<R, W> on Indexable<R, W> {\n\
                   \x20 R operator [](int index) => _value;\n\
                   \x20 void operator []=(int index, W value) {}\n\
                   }\n\
                   Indexable<R, W>? maybeIndexable<R, W>(R v) => Indexable<R, W>(v);\n\
                   void f() {\n\
                   \x20 maybeIndexable<String, int>('')?[0] ??= 1;\n\
                   }\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    let unused: Vec<_> = spans_of(&report, &full, "unused_element");
    assert!(
        unused.iter().any(|s| s == "[]"),
        "[] reports: the null-aware ??= credits the write element []= alone"
    );
    assert!(
        !unused.iter().any(|s| s == "[]="),
        "[]= silent: the null-aware write's writeOrReadElement is []="
    );
}

/// The pure read `var y = x[0]` is the mirror: `[]=` reports, `[]`
/// silent (oracle-verified on the same fixture shape).
#[test]
fn index_read_credits_only_index_get() {
    let fixture = "class Indexable<R, W> {\n\
                   \x20 final R _value;\n\
                   \x20 Indexable(this._value);\n\
                   }\n\
                   extension<R, W> on Indexable<R, W> {\n\
                   \x20 R operator [](int index) => _value;\n\
                   \x20 void operator []=(int index, W value) {}\n\
                   }\n\
                   void f() {\n\
                   \x20 var x = Indexable<String, int>('');\n\
                   \x20 var y = x[0];\n\
                   \x20 y;\n\
                   }\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    let unused: Vec<_> = spans_of(&report, &full, "unused_element");
    assert!(
        unused.iter().any(|s| s == "[]="),
        "[]= reports: a pure read credits only []"
    );
    assert!(
        !unused.iter().any(|s| s == "[]"),
        "[] silent: the read's writeOrReadElement is []"
    );
}

// -- `catch (late e)` ---------------------------------------------------
//
// `late` inside a catch clause's parenthesized head is a modifier
// keyword, not a binder: `retire_keyword_named_catches` drops the
// staged `UnusedCatchClause`/`unused_local_variable` row the
// clause-head scan finds `late` as its first identifier.

/// `catch (late e) {}` — `e` is the clause binder but `late` is a
/// keyword, so the clause reports nothing.
#[test]
fn catch_late_keyword_silent() {
    let fixture = "void f() {\n\
                   \x20 try {\n\
                   \x20 } catch (late e) {\n\
                   \x20 }\n\
                   }\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert!(
        spans_of(&report, &full, "unused_catch_clause").is_empty()
            && spans_of(&report, &full, "unused_local_variable")
                .iter()
                .all(|s| s != "e"),
        "catch (late e) silent: late is a modifier, not the binder"
    );
}

// -- Used-override spare ------------------------------------------------
//
// The super-walk now asks `overrides_used_element` whether the
// overridden member is used, not merely whether it exists. A chain
// `C` declares `_f`, `B extends C` reads `_f` (implicit `this`), `D
// extends B` shadows `_f` — `B`'s read makes `C._f` used, and the
// recursive walk through `D`'s super edge lands on `C._f`'s used
// element, sparing `D._f`.

/// `C._f` used through `B extends C`'s implicit `this._f` read;
/// `D extends B` redeclares `_f` — D's `_f` is still spared because the
/// overridden element it hides is read.
#[test]
fn subclass_shadowing_member_spared_by_used_super() {
    let fixture = "class C {\n\
                   \x20 int? _f;\n\
                   }\n\
                   class B extends C {\n\
                   \x20 void m() { _f; }\n\
                   }\n\
                   class D extends B {\n\
                   \x20 int? _f;\n\
                   }\n\
                   void f() { B().m(); }\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    // C._f is read by B.m; D._f hides it but the overridden element is
    // used — D._f stays silent.
    let unused: Vec<_> = spans_of(&report, &full, "unused_element");
    assert!(
        !unused.iter().any(|s| s == "_f"),
        "D._f silent: overridesUsedElement walks through to C._f which is used"
    );
}

// -- Duplicate enum constructor ------------------------------------------
//
// Upstream `constructors.length` counts duplicate selectors: two
// `E.foo` sites collapse to one name but the model carries both sites.
// The `site_count`/`earlier_dup` arms keep the LAST duplicate's row
// (the one the official's `unused_element` anchor lands on).

/// `enum E { v; E.foo(); E.foo(); }` — the second `foo` reports, the
/// first is spared as the earlier duplicate.
#[test]
fn duplicate_enum_constructor_reports_at_last_site() {
    let fixture = "enum E {\n\
                   \x20 v;\n\
                   \x20 const E();\n\
                   \x20 const E.foo();\n\
                   \x20 const E.foo();\n\
                   }\n\
                   void f() { E.v; }\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    let unused: Vec<_> = spans_of(&report, &full, "unused_element");
    assert!(
        unused.iter().any(|s| s == "foo"),
        "duplicate E.foo reports at the last site"
    );
}

// -- Unresolved-URI `''` arm ---------------------------------------------
//
// `import '';` — the parser hands the directive a `DirectiveUriWithout
// String` face whose `relativeUriString` is `''`; the edge's target is
// `None` and the report anchors the URI literal's range with the empty
// `{0}`. The `edge.target == None` arm reports `''` whenever the site
// carries no written text; a named-but-missing target (`import
// 'x.dart'` where `x.dart` doesn't exist) keeps its non-empty
// `uri_text` and the `uri_does_not_exist` face's row — `unused_import`
// stays silent on that face. Verified end-to-end against the release
// binary: `dartq analyze` on `import '';` reports
// `Unused import: ''.` at 1:8 len 2, byte-identical to the oracle.
//
// The harness cannot express the `None`-target edge itself — the rig's
// `LibraryGraphBuilder` synthesizes a `Some(target)` edge for every
// `RigImport`, so the count gate
// (`edges.len() != self.imports.len()`) drops the pass before the
// unresolved arm runs. The pins below cover the reachable half of the
// same function: a wholly unused resolved import still reports its
// written URI, and a used one stays silent.

/// An imported library nothing reads reports `unused_import` with the
/// written URI text as `{0}` — the resolved-target arm the `''` arm
/// sits beside.
#[test]
fn unused_resolved_import_reports_uri() {
    let fixture = "import 'package:t/lib.dart';\n\
                   void f() {}\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops_imports(
        fixture,
        &[RigImport {
            uri: "package:t/lib.dart",
            prefix: None,
            source: "class Marker {}\n",
        }],
    );
    let unused: Vec<_> = spans_of(&report, &full, "unused_import");
    assert_eq!(
        unused,
        vec!["'package:t/lib.dart'".to_string()],
        "an unused resolved import reports its URI literal"
    );
}

/// A use through the import's element keeps the edge used — the
/// `is_used` verdict the unresolved arm never reaches.
#[test]
fn used_resolved_import_silent() {
    let fixture = "import 'package:t/lib.dart';\n\
                   void f() { Marker(); }\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops_imports(
        fixture,
        &[RigImport {
            uri: "package:t/lib.dart",
            prefix: None,
            source: "class Marker {}\n",
        }],
    );
    assert!(
        spans_of(&report, &full, "unused_import").is_empty(),
        "a used import stays silent"
    );
}

// -- named extension negative control -------------------------------------
//
// `is_publicly_accessible`'s Extension arm answers `enclosingElement
// .isPublic` (unused_local_elements_verifier.dart:870-873): a NAMED
// public extension's members are publicly accessible, so the same
// operator pair that reports inside an unnamed extension stays silent
// here — oracle-verified zero rows on this fixture.

/// `extension Named on Indexable<int, int>` with `x[0] ??= 1` — the
/// named-public container keeps `[]`/`[]=` publicly accessible, so
/// `unused_element` reports nothing (the unnamed-extension twin reports
/// `[]`).
#[test]
fn named_extension_operators_silent() {
    let fixture = "class Indexable<R, W> {\n\
                   \x20 final R _value;\n\
                   \x20 Indexable(this._value);\n\
                   }\n\
                   extension Named on Indexable<int, int> {\n\
                   \x20 int operator [](int index) => _value;\n\
                   \x20 void operator []=(int index, int value) {}\n\
                   }\n\
                   void f() {\n\
                   \x20 var x = Indexable<int, int>(0);\n\
                   \x20 x[0] ??= 1;\n\
                   }\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert!(
        spans_of(&report, &full, "unused_element").is_empty(),
        "a named public extension's members stay publicly accessible — no \
         unused_element rows"
    );
}
