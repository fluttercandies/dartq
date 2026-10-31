//! #3241 pins — the `unused_element` used-credit lattice: the shadowed
//! top-level `_` face plus the used-credit joins the #3223 recon mapped onto
//! `finalize_usage` (clusters C4-C6).
//!
//! The shadowed face. A top-level `_` is an ORDINARY private element: the
//! wildcard kinds are local variables, local functions, prefixes, type
//! parameters, and formals only (`isWildcardVariable`, upstream Dart SDK
//! source, pkg/analyzer/lib/src/dart/element/extensions.dart:81-91), so the
//! unused-element verifier visits a top-level `_` like any private name
//! (`_visitTopLevelVariableElement` → `_isUsedElement`,
//! unused_local_elements_verifier.dart:1161-1168/:910-975). The library
//! scope binds each spelling to its FIRST declaration (`_getters[id] ??=
//! element`, scope.dart:880-885): a later same-name declaration is never
//! reachable by name, so no read can credit its element, and it reports
//! whenever it is private. dartq's `top_level_used` set is name-level — one
//! read of `_` spared every same-spelled site, the shadowed twin included —
//! so the second `_` never reported; `finalize_usage` now claims each full
//! site spelling once, in source order, and a later claimant skips the
//! name-level credit. The #3229 local-scope exemption is a different face
//! (the locals pass) and stays untouched.
//!
//! The used-credit joins. `GatherUsedLocalElementsVisitor` credits a member
//! on every resolved access (`usedElements.members`, :341-348) including
//! pattern fields (`visitPatternField` :249-253), an operator or index use
//! (`visitBinaryExpression`/`visitIndexExpression`/`visitPostfixExpression`
//! :59-64/:196-201/:256-267 → `usedElements.addMember`), an implicit `call`
//! (`visitFunctionExpressionInvocation` :175-180), and a write's
//! `writeElement` (:304-308 → the setter accessor). dartq's element-level
//! pool (`member_refs`) missed the pattern-field and operator shapes and the
//! top-level setter write; the resolve layer leaves an `extension on
//! Function` application over a function-valued receiver untyped. The
//! finalization pass now derives those credits from the row and ref pools
//! it already holds.
//!
//! Registered through `diagnose/tests.rs`'s `#[path]` block (`diagnose.rs`
//! is a live-lane file out of this ticket's write scope; `#[path]` keeps the
//! module inside the `diagnose` subtree so `crate::diagnose::test_rig`
//! resolves).

use crate::diagnose::test_rig::{CORE_PRELUDE, report_ops_wildcards, rows};
use crate::diagnose::types::FileReport;

/// The offsets of the `unused_element` rows, ascending.
fn unused_element_offsets(report: &FileReport) -> Vec<u32> {
    let mut at = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unused_element")
        .map(|d| d.range.offset)
        .collect::<Vec<_>>();
    at.sort_unstable();
    at
}

/// The absolute offset of the `nth` occurrence (0-based) of `needle` in the
/// fixture as the rig analyzes it: `report_ops*` prefixes [`CORE_PRELUDE`],
/// so every fixture-local offset shifts by the prelude's length.
fn at(fixture: &str, needle: &str, nth: usize) -> u32 {
    let mut from = 0;
    for _ in 0..nth {
        from += fixture[from..]
            .find(needle)
            .expect("needle appears in the fixture")
            + needle.len();
    }
    let hit = from
        + fixture[from..]
            .find(needle)
            .expect("needle appears in the fixture");
    u32::try_from(CORE_PRELUDE.len() + hit).expect("fixture offsets fit u32")
}

// —— the shadowed top-level `_` (#3241) ————————————————————————————————————

/// The brief's shape: a second top-level `_` is shadowed by the first (the
/// scope's `??=`), so a read of `_` credits the FIRST element alone and the
/// second — unreachable by name — reports `unused_element`.
#[test]
fn shadowed_top_level_wildcard_reports_the_second_declaration_only() {
    let fixture = "\
var _ = 7;
int _ = 3;
void f() {
  var y = _;
}
";
    let report = report_ops_wildcards(fixture, true);
    assert_eq!(
        unused_element_offsets(&report),
        vec![at(fixture, "int _", 0) + 4],
        "the read binds the first `_`; the shadowed second reports: {}",
        rows(&report).join("\n")
    );
}

/// Without any read both top-level `_` elements are unused private
/// elements: the first reports on its own merits, the second as the
/// shadowed twin (the oracle's `_visitTopLevelVariableElement` runs on each
/// variable's element).
#[test]
fn shadowed_top_level_wildcard_without_reads_reports_both() {
    let fixture = "\
var _ = 7;
int _ = 3;
";
    let report = report_ops_wildcards(fixture, true);
    assert_eq!(
        unused_element_offsets(&report),
        vec![at(fixture, "var _", 0) + 4, at(fixture, "int _", 0) + 4],
        "both unused top-level `_` elements report: {}",
        rows(&report).join("\n")
    );
}

/// A lone unused top-level `_` is an ordinary private element and reports.
#[test]
fn unused_top_level_wildcard_reports() {
    let fixture = "\
var _ = 7;
";
    let report = report_ops_wildcards(fixture, true);
    assert_eq!(
        unused_element_offsets(&report),
        vec![at(fixture, "_", 0)],
        "a top-level `_` is no wildcard: {}",
        rows(&report).join("\n")
    );
}

/// A used top-level `_` stays silent.
#[test]
fn used_top_level_wildcard_is_silent() {
    let fixture = "\
var _ = 1;
void f() {
  var y = _;
}
";
    let report = report_ops_wildcards(fixture, true);
    assert!(
        unused_element_offsets(&report).is_empty(),
        "the read credits the top-level `_`: {}",
        rows(&report).join("\n")
    );
}

/// The #3229 local-scope exemption is untouched: a `_` formal declares
/// nothing and never reports an unused-element row of any kind.
#[test]
fn local_wildcard_formal_stays_exempt() {
    let fixture = "\
void f(int _) {
  var y = 1;
}
";
    let report = report_ops_wildcards(fixture, true);
    assert!(
        unused_element_offsets(&report).is_empty(),
        "a local `_` is a wildcard and never reports: {}",
        rows(&report).join("\n")
    );
    assert!(
        report
            .diagnostics
            .iter()
            .all(|d| d.code.name != "unused_element_parameter"),
        "a wildcard formal is unnamed, so no parameter row either: {}",
        rows(&report).join("\n")
    );
}

/// A CLASS MEMBER named `_` is not a wildcard kind either
/// (MethodDeclarationResolutionTest.test_wildCardMethod): the unused
/// private method reports under both feature postures.
#[test]
fn underscore_method_reports_under_both_postures() {
    let fixture = "\
class C {
  _() {}
}
";
    let expected = vec![at(fixture, "_()", 0)];
    let enabled = report_ops_wildcards(fixture, true);
    assert_eq!(
        unused_element_offsets(&enabled),
        expected,
        "a `_` method is an ordinary private member: {}",
        rows(&enabled).join("\n")
    );
    let disabled = report_ops_wildcards(fixture, false);
    assert_eq!(
        unused_element_offsets(&disabled),
        expected,
        "the pre-feature posture reports the same row: {}",
        rows(&disabled).join("\n")
    );
}

// —— C6b: the top-level setter's write credit ——————————————————————————————

/// `other_declarations_A02_t05`: a write through a standalone top-level
/// setter credits the setter accessor (`_useIdentifierElement(node
/// .writeElement)`, unused_local_elements_verifier.dart:304-308), so the
/// setter stays silent.
#[test]
fn top_level_setter_write_credits_the_setter() {
    let fixture = "\
void set _(int v) {}
void main() {
  _ = 42;
}
";
    let report = report_ops_wildcards(fixture, true);
    assert!(
        unused_element_offsets(&report).is_empty(),
        "the write credits the setter element: {}",
        rows(&report).join("\n")
    );
}

/// The write credit is the SETTER's alone: a write-only top-level variable
/// still reports (the variable element is never added to `usedElements`
/// by a write — only its induced setter accessor is).
#[test]
fn write_only_top_level_variable_still_reports() {
    let fixture = "\
int _v = 0;
void main() {
  _v = 42;
}
";
    let report = report_ops_wildcards(fixture, true);
    assert_eq!(
        unused_element_offsets(&report),
        vec![at(fixture, "_v", 0)],
        "a write never credits the variable element itself: {}",
        rows(&report).join("\n")
    );
}

// —— C6a: the pattern-field read credit ————————————————————————————————————

/// `Patterns/wildcards_A03_t01`: an object pattern's field `_: 42` reads
/// the getter `_` (`visitPatternField` → `addMember`/`addReadMember`), so
/// the private getter is used.
#[test]
fn object_pattern_field_read_credits_the_getter() {
    let fixture = "\
class C {
  int get _ => 42;
}
String f(C c) {
  switch (c) {
    case C(_: 42):
      return \"match\";
    default:
      return \"no\";
  }
}
";
    let report = report_ops_wildcards(fixture, true);
    assert!(
        unused_element_offsets(&report).is_empty(),
        "the pattern field reads the getter: {}",
        rows(&report).join("\n")
    );
}

// —— C5: operator / index / `call` uses credit the extension member ————————

/// UnusedElementTest.test_method_isUsed_privateExtension_indexOperator: an
/// index read over the extended type credits the extension's `[]`.
#[test]
fn extension_index_operator_use_credits_the_member() {
    let fixture = "\
extension _A on bool {
  int operator [](int index) => 7;
}
void main() {
  false[3];
}
";
    let report = report_ops_wildcards(fixture, true);
    assert!(
        unused_element_offsets(&report).is_empty(),
        "`false[3]` applies `_A.[]`: {}",
        rows(&report).join("\n")
    );
}

/// test_method_isUsed_privateExtension_indexEqOperator: an index write
/// credits the extension's `[]=`.
#[test]
fn extension_index_write_credits_the_member() {
    let fixture = "\
extension _A on bool {
  operator []=(int index, int value) {}
}
void main() {
  false[0] = 1;
}
";
    let report = report_ops_wildcards(fixture, true);
    assert!(
        unused_element_offsets(&report).is_empty(),
        "`false[0] = 1` applies `_A.[]=`: {}",
        rows(&report).join("\n")
    );
}

/// test_method_isUsed_privateExtension_operator_assignment: a compound
/// assignment over the extended type credits the binary operator.
#[test]
fn extension_compound_operator_use_credits_the_member() {
    let fixture = "\
extension _A on String {
  String operator -(int other) => this;
}
void f(String s) {
  s -= 3;
}
";
    let report = report_ops_wildcards(fixture, true);
    assert!(
        unused_element_offsets(&report).is_empty(),
        "`s -= 3` applies `_A.-`: {}",
        rows(&report).join("\n")
    );
}

/// test_method_isUsed_privateExtension_methodCall: invoking a value whose
/// extension declares `call` credits that member.
#[test]
fn extension_call_use_credits_the_member() {
    let fixture = "\
extension _E on int {
  void call() {}
}
void f() {
  0();
}
";
    let report = report_ops_wildcards(fixture, true);
    assert!(
        unused_element_offsets(&report).is_empty(),
        "`0()` applies `_E.call`: {}",
        rows(&report).join("\n")
    );
}

/// The no-over-credit control: an extension operator the unit never
/// applies keeps its row.
#[test]
fn unapplied_extension_operator_still_reports() {
    let fixture = "\
extension _A on bool {
  int operator [](int index) => 7;
}
void main() {
  var b = true;
  b = !b;
}
";
    let report = report_ops_wildcards(fixture, true);
    assert_eq!(
        unused_element_offsets(&report),
        vec![at(fixture, "[]", 0)],
        "no index access, so `_A.[]` stays unused: {}",
        rows(&report).join("\n")
    );
}

// —— C4: an `extension on Function` applied to a function-valued receiver —

/// FunctionReferenceResolutionTest.test_topLevelFunction_targetOfFunctionCall:
/// `foo<int>.m()` instantiates the generic function and applies the
/// `on Function` extension — the member is credited
/// (`visitSimpleIdentifier`'s `ExtensionElement` arm, :341-348).
#[test]
fn extension_on_function_credits_a_function_receiver_member() {
    let fixture = "\
void foo<T>(T arg) {}

extension on Function {
  void m() {}
}
void bar() {
  foo<int>.m();
}
";
    let report = report_ops_wildcards(fixture, true);
    assert!(
        unused_element_offsets(&report).is_empty(),
        "`foo<int>.m()` applies the `on Function` face: {}",
        rows(&report).join("\n")
    );
}

/// test_function_extensionOnFunction: the tear-off read `foo.m<int>` over
/// a plain function reference applies the same face.
#[test]
fn extension_on_function_credits_a_plain_function_tearoff() {
    let fixture = "\
void foo<T>(T a) {}

void bar() {
  foo.m<int>;
}

extension on Function {
  void m<T>(T t) {}
}
";
    let report = report_ops_wildcards(fixture, true);
    assert!(
        unused_element_offsets(&report).is_empty(),
        "`foo.m<int>` applies the `on Function` face: {}",
        rows(&report).join("\n")
    );
}
