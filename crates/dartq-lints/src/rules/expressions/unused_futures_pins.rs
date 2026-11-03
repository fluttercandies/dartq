use crate::testsupport::{Fixture, one_rule};

/// The #3776 shared-walk pins: `discarded_futures` and
/// `unawaited_futures` oracle shapes through the production bridge
/// (the semantic face mounted — both rules read it). Every pin cites
/// the conformance row it mirrors (`conformance/expected.tsv`,
/// extracted from the pinned `dart analyze` 3.13.7 oracle), so the
/// conformance diff and the pin assert the same verdict.

/// One report row as the oracle renders it: 1-based line, 1-based
/// UTF-16 column, UTF-16 length.
type Row = (u32, u32, u32);

fn rows_of(source: &str, rule: &str) -> Vec<Row> {
    let f = Fixture::parse(source);
    let mut rows: Vec<Row> = f
        .run_semantic(&one_rule(rule))
        .iter()
        .map(|lint| {
            let (line, col) = f.lines.line_col_utf16(&f.source, lint.range.start);
            (line, col, lint.range.end - lint.range.start)
        })
        .collect();
    rows.sort_unstable();
    rows
}

/// `test_instanceProperty_unawaited#0` — `INFO|unawaited_futures|
/// lib/test.dart|3:5|1` — the `c.p;` statement reports at the property
/// name, not at the zero-width close after `;` (the `NoArguments`
/// tear-off send plus trailing `DotAccess` marker tiles: the marker's
/// `Operator` word is the dot, the name is the token after it).
#[test]
fn unawaited_futures_property_access_reports_at_the_name() {
    let source = "void f(C c) async {\n  c.p;\n}\nabstract class C {\n  Future<int> get p;\n}\n";
    assert_eq!(rows_of(source, "unawaited_futures"), vec![(2, 5, 1)]);
}

/// The same property-access anchor on the sync side — the shared
/// walk's `Send`-arm fallback covers both rules.
#[test]
fn discarded_futures_property_access_reports_at_the_name() {
    let source = "void f(C c) {\n  c.p;\n}\nabstract class C {\n  Future<int> get p;\n}\n";
    assert_eq!(rows_of(source, "discarded_futures"), vec![(2, 5, 1)]);
}

/// `test_functionCall_classImplementsFuture#0`'s lint row —
/// `INFO|unawaited_futures|lib/test.dart|3:3|1` — a static type whose
/// written supertype chain lands on the core `Future` name is
/// Future-relevant (the upstream `asInstanceOf(futureElement)` gate;
/// `abstract class Future2 implements Future {}`). The case stays
/// conformance-red on a dartq-semantic `implements_non_class` false
/// positive the semantic face owns — this pin covers the lint side
/// only.
#[test]
fn unawaited_futures_future_subtype_reports() {
    let source = "void f(Future2 p) async {\n  g(p);\n}\nFuture2 g(Future2 p) => p;\nabstract class Future2 implements Future {}\n";
    assert_eq!(rows_of(source, "unawaited_futures"), vec![(2, 3, 1)]);
}

/// The `FutureOr` written name lands on the same 3.13.7 code row —
/// the paired `*FutureOr` diagnostic texts are a later-SDK face, not
/// this pin (`test_function_futureOr#0`: `INFO|discarded_futures|
/// lib/test.dart|5:3|1|'Future'-returning calls…`).
#[test]
fn discarded_futures_future_or_reports_the_single_code_row() {
    let source = "import 'dart:async';\n\nvoid f() {\n  g();\n}\n\nFutureOr<int> g() async => 0;\n";
    let lints = Fixture::parse(source).run_semantic(&one_rule("discarded_futures"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "'Future'-returning calls in a non-'async' function."
    );
}

/// `test_function_awaitNotRequired#0` — `-` — a bare call whose
/// resolved top-level declaration carries the annotation's metadata
/// star stays silent (the official `element.metadata.hasAwaitNotRequired`
/// face).
#[test]
fn discarded_futures_await_not_required_function_stays_silent() {
    let source = "import 'package:meta/meta.dart';\nvoid f() {\n  g();\n}\n\n@awaitNotRequired\nFuture<int> g() async => 7;\n";
    assert!(
        Fixture::parse(source)
            .run_semantic(&one_rule("discarded_futures"))
            .is_empty()
    );
    // The async twin reads the same declaration star.
    let source = "import 'package:meta/meta.dart';\nvoid f() async {\n  g();\n}\n@awaitNotRequired\nFuture<int> g() => Future.value(0);\n";
    assert!(
        Fixture::parse(source)
            .run_semantic(&one_rule("unawaited_futures"))
            .is_empty()
    );
}

/// `test_method_awaitNotRequired#0` — `-` — a bare call whose resolved
/// member of the enclosing class-like carries the annotation stays
/// silent (the implicit-`this` member face).
#[test]
fn discarded_futures_await_not_required_method_stays_silent() {
    let source = "import 'package:meta/meta.dart';\nclass C {\n  void m() {\n    g();\n  }\n\n  @awaitNotRequired\n  Future<void> g() async {}\n}\n";
    assert!(
        Fixture::parse(source)
            .run_semantic(&one_rule("discarded_futures"))
            .is_empty()
    );
}

/// `test_instanceProperty_unawaited_awaitNotRequired#0` — `-` — a
/// property access whose resolved getter carries the annotation stays
/// silent (the `member_of` entry's own star).
#[test]
fn unawaited_futures_await_not_required_getter_stays_silent() {
    let source = "import 'package:meta/meta.dart';\nvoid f(C c) async {\n  c.p;\n}\nabstract class C {\n  @awaitNotRequired\n  Future<int> get p;\n}\n";
    assert!(
        Fixture::parse(source)
            .run_semantic(&one_rule("unawaited_futures"))
            .is_empty()
    );
}

/// `test_instanceProperty_unawaited_awaitNotRequiredInherited#0` —
/// `-` — the receiver's own type declares `p` unannotated, but the
/// same-file superclass declares the annotated getter (the upstream
/// `superMembers.any(hasAwaitNotRequired)` walk).
#[test]
fn unawaited_futures_await_not_required_inherited_stays_silent() {
    let source = "import 'package:meta/meta.dart';\nvoid f(D d) async {\n  d.p;\n}\nabstract class C {\n  @awaitNotRequired\n  Future<int> get p;\n}\nclass D extends C {\n  Future<int> p = Future.value(7);\n}\n";
    assert!(
        Fixture::parse(source)
            .run_semantic(&one_rule("unawaited_futures"))
            .is_empty()
    );
}

/// The escapes do not over-reach: an unannotated member or
/// declaration still reports, and an annotated unrelated member does
/// not silence the dropped call.
#[test]
fn await_not_required_is_member_exact() {
    // A same-named but unannotated member still reports.
    let source = "void f(C c) async {\n  c.p;\n}\nabstract class C {\n  Future<int> get p;\n}\n";
    assert_eq!(rows_of(source, "unawaited_futures"), vec![(2, 5, 1)]);
    // An annotation on a DIFFERENT member does not silence `p`. The
    // `meta` import puts the read on line 3 (`INFO|unawaited_futures|
    // lib/test.dart|3:5|1` — the property name, one UTF-16 unit).
    let source = "import 'package:meta/meta.dart';\nvoid f(C c) async {\n  c.p;\n}\nabstract class C {\n  @awaitNotRequired\n  Future<int> get q;\n  Future<int> get p;\n}\n";
    assert_eq!(rows_of(source, "unawaited_futures"), vec![(3, 5, 1)]);
    // An unannotated top-level callee still reports.
    let source = "import 'package:meta/meta.dart';\nvoid f() async {\n  g();\n}\nFuture<int> g() => Future.value(0);\n";
    assert_eq!(rows_of(source, "unawaited_futures"), vec![(3, 3, 1)]);
}
