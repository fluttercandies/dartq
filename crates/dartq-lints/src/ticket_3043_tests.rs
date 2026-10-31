use crate::engine::{Lint, ReachabilityFace};
use crate::testsupport::{Fixture, one_rule, tok, utf16_at};

/// The #3043 lint pins: one green pin per fixed shape, each citing the
/// upstream arm it mirrors (pinned `dart analyze` 3.13.7 oracle
/// semantics in `conformance/expected.tsv`).

// —— unreachable_from_main ——————————————————————————————————————————
// Upstream Dart SDK source, pkg/linter
// .../unreachable_from_main.dart — the constructor marking arms
// (:199-215 default-constructor walk, :220-231 `@reflectiveTest`,
// :247-259 implicit-super, :400-406 unnamed `super()`).

/// The `(nth-occurrence UTF-16 offset, reachable)` face flag of `name`.
fn flag(f: &Fixture, name: &str, nth: usize, reachable: bool) -> (u32, bool) {
    (utf16_at(f, tok(&f.source, &f.tokens, name, nth)), reachable)
}

/// Runs the rule with the given face mounted (flags sorted ascending,
/// the seam contract).
fn run(f: &Fixture, has_entry: bool, entries: &[(u32, bool)]) -> Vec<Lint> {
    let mut flags = entries.to_vec();
    flags.sort_unstable_by_key(|&(offset, _)| offset);
    let face = ReachabilityFace {
        has_entry,
        flags: &flags,
    };
    f.run_with(&one_rule("unreachable_from_main"), None, Some(&face), None)
}

/// The constructor verdict fallback: the face universe carries no
/// constructor rows (the `dartq-index` capture exclusion), so an
/// unreferenced constructor answers no verdict and its own reference
/// face decides. Oracle-verified (`dart analyze`, mirroring upstream
/// `test_constructor_named_unreachable`): a live class through the
/// bare expression use `C;` leaves an unreferenced constructor
/// reporting (`C.named` / `C.new`) — under a DEAD class the container
/// wins and the constructor never reports.
#[test]
fn ufm_ctor_verdict_fallback_reports() {
    let f = Fixture::parse("void main() {\n  C;\n}\n\nclass C {\n  C();\n}\n");
    let lints = run(&f, true, &[flag(&f, "main", 0, true), flag(&f, "C", 0, true)]);
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Unreachable member 'C.new' in an executable library."
    );
    assert_eq!(lints[0].range.start, flag(&f, "C", 2, false).0);

    let f = Fixture::parse("void main() {\n  C;\n}\n\nclass C {\n  C.named();\n}\n");
    let lints = run(&f, true, &[flag(&f, "main", 0, true), flag(&f, "C", 0, true)]);
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Unreachable member 'C.named' in an executable library."
    );

    let f = Fixture::parse("void main() {}\n\nclass C {\n  C();\n}\n");
    let lints = run(&f, true, &[flag(&f, "main", 0, true), flag(&f, "C", 0, true)]);
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Unreachable member 'C' in an executable library."
    );
    assert_eq!(lints[0].range.start, flag(&f, "C", 0, false).0);
}

/// A counting position keeps the constructor silent even under a
/// flagged-unreachable class (the anti-reach face): `C()` in `main`
/// counts both the bare name and `C.new`.
#[test]
fn ufm_counted_invocation_suppresses_ctor() {
    let f = Fixture::parse("void main() {\n  C();\n}\n\nclass C {\n  C();\n}\n");
    let lints = run(
        &f,
        true,
        &[flag(&f, "main", 0, true), flag(&f, "C", 0, false)],
    );
    assert!(lints.is_empty(), "{lints:?}");
}

/// The implicit-super arm (:199-215, :454-465): `B()` walks `B`'s
/// default constructor, whose implicit super-initializer marks `S`'s
/// unnamed constructor — `S()` stays silent.
#[test]
fn ufm_subclass_default_ctor_marks_super_unnamed() {
    let f = Fixture::parse("void main() {\n  B();\n}\n\nclass S {\n  S();\n}\n\nclass B extends S {}\n");
    let lints = run(
        &f,
        true,
        &[
            flag(&f, "main", 0, true),
            flag(&f, "S", 0, true),
            flag(&f, "B", 0, true),
        ],
    );
    assert!(lints.is_empty(), "{lints:?}");
}

/// The explicit named-super initializer (`super.named()`) forecloses
/// the implicit arm (:250-253 `hasSuperInitializer`) — the supertype's
/// UNNAMED constructor stays dead and reports, while the named one is
/// counted through the `super.named()` continuation.
#[test]
fn ufm_named_super_initializer_leaves_super_unnamed_dead() {
    let f = Fixture::parse(
        "void main() {\n  B();\n}\n\nclass S {\n  S();\n  S.named();\n}\n\nclass B extends S {\n  B() : super.named();\n}\n",
    );
    let lints = run(
        &f,
        true,
        &[
            flag(&f, "main", 0, true),
            flag(&f, "S", 0, true),
            flag(&f, "B", 0, true),
        ],
    );
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Unreachable member 'S.new' in an executable library."
    );
}

/// The `@reflectiveTest` arm (:220-231): the mirrors instantiation
/// marks the unnamed constructor — the upstream
/// `test_constructor_reachableViaTestReflectiveLoader` fixture
/// (`assertNoDiagnostics`) with the bare expression use `A;` keeping
/// the class alive. Oracle note: the arm keys the annotation spelling
/// (the element face resolves `package:test_reflective_loader` — a
/// corpus library carries the import).
#[test]
fn ufm_reflective_test_marks_unnamed_ctor() {
    let f = Fixture::parse(
        "import 'package:test_reflective_loader/test_reflective_loader.dart';\n\nvoid main() {\n  A;\n}\n\n@reflectiveTest\nclass A {\n  A();\n}\n",
    );
    let lints = run(
        &f,
        true,
        &[
            flag(&f, "main", 0, true),
            flag(&f, "A", 0, true),
            flag(&f, "A", 1, false),
        ],
    );
    assert!(lints.is_empty(), "{lints:?}");
}

// —— prefer_initializing_formals —————————————————————————————————————
// Upstream Dart SDK source, pkg/linter
// .../prefer_initializing_formals.dart — the named-section descent and
// the `private-named-parameters` gate (:167-182, shipped 3.12.0).

/// A `{named}` parameter converts: the initializer face reads the
/// section parameters (the `OptionalFormalParameters` descent).
#[test]
fn pif_named_parameter_convertible_reports() {
    let f = Fixture::parse("class C {\n  int x;\n  C({int x}) : x = x;\n}\n");
    let lints = f.run_semantic(&one_rule("prefer_initializing_formals"));
    assert_eq!(lints.len(), 1, "{lints:?}");
}

/// The private-field face keys on the shipped
/// `private-named-parameters` feature: no settled version (shipped
/// default) and 3.12 lint, 3.11 stays silent.
#[test]
fn pif_private_field_version_gate() {
    let source = "class C {\n  int _x;\n  C({int x}) : _x = x;\n}\n";
    let mut f = Fixture::parse(source);
    assert_eq!(
        f.run_semantic(&one_rule("prefer_initializing_formals"))
            .len(),
        1
    );
    f.version = Some(dartq_language::LanguageVersion::new(3, 11));
    assert!(
        f.run_semantic(&one_rule("prefer_initializing_formals"))
            .is_empty()
    );
    f.version = Some(dartq_language::LanguageVersion::new(3, 12));
    assert_eq!(
        f.run_semantic(&one_rule("prefer_initializing_formals"))
            .len(),
        1
    );
}

/// The private face converts from a NAMED parameter only
/// (`parameter.isPositional` returns) — a positional parameter over a
/// private field stays silent at 3.12.
#[test]
fn pif_private_field_positional_stays_silent() {
    let mut f = Fixture::parse("class C {\n  int _x;\n  C(int x) : _x = x;\n}\n");
    f.version = Some(dartq_language::LanguageVersion::new(3, 12));
    assert!(
        f.run_semantic(&one_rule("prefer_initializing_formals"))
            .is_empty()
    );
}

// —— avoid_dynamic_calls —————————————————————————————————————————————
// Upstream Dart SDK source, pkg/linter
// .../avoid_dynamic_calls.dart — the `realTarget` receiver extent
// (:116 `reportAtNode(target)`) and the cascade-section `.call` face
// (:119-129).

/// A multi-piece receiver chain reports at the whole `realTarget`
/// (`a.b` — upstream `visitPropertyAccess` reports the target, not its
/// last selector).
#[test]
fn adc_member_chain_receiver_reports_whole() {
    let f = Fixture::parse("void f(A a) {\n  a.b.foo;\n}\n\nclass A {\n  dynamic b = 1;\n}\n");
    let lints = f.run_semantic(&one_rule("avoid_dynamic_calls"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let a = tok(&f.source, &f.tokens, "a", 1);
    assert_eq!(lints[0].range.start, utf16_at(&f, a));
    assert_eq!(lints[0].range.end - lints[0].range.start, 3);
}

/// A `..call()` section over a bare-`Function` target reports at the
/// `call` selector (the `.call` special case rides the cascade
/// sections, each a `MethodInvocation` over the same target upstream).
#[test]
fn adc_cascade_call_on_bare_function_reports_at_call() {
    let f = Fixture::parse("void f(Function p) {\n  p..call();\n}\n");
    let lints = f.run_semantic(&one_rule("avoid_dynamic_calls"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let call = tok(&f.source, &f.tokens, "call", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, call));
    assert_eq!(lints[0].range.end - lints[0].range.start, 4);
}

/// A concrete function type is not `isDartCoreFunction` — the section
/// stays silent (upstream `test_callInvocation_functionType`).
#[test]
fn adc_cascade_call_on_concrete_function_stays_silent() {
    let f = Fixture::parse("void f(void Function() p) {\n  p..call();\n}\n");
    let lints = f.run_semantic(&one_rule("avoid_dynamic_calls"));
    assert!(lints.is_empty(), "{lints:?}");
}

// —— specify_nonobvious_property_types ————————————————————————————————
// Upstream Dart SDK source, pkg/linter/lib/src/util/obvious_types.dart
// (:166-179 the `SimpleIdentifier` arm — locals and formals only, and
// neither occurs at a top-level or field site).

/// A bare-identifier initializer is never obvious at these sites — the
/// top-level and static-field faces report.
#[test]
fn snp_bare_identifier_initializer_needs_type() {
    let f = Fixture::parse("int y = 1;\nvar x = y;\n");
    let lints = f.run(&one_rule("specify_nonobvious_property_types"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "A type annotation is needed because it isn't obvious."
    );
    let var = tok(&f.source, &f.tokens, "var", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, var));

    let f = Fixture::parse("class C {\n  static int y = 1;\n  var x = y;\n}\n");
    let lints = f.run(&one_rule("specify_nonobvious_property_types"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let var = tok(&f.source, &f.tokens, "var", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, var));
}

/// The obvious faces keep their silences under the rule-local
/// identifier gate: literals and the parenthesized literal. (The
/// MINUS-prefix and typed-literal faces answer through the shared
/// spelling helper, unchanged by this ticket — their silences are the
/// family's conformance residuals, not shapes this wave fixed.)
#[test]
fn snp_obvious_initializers_stay_silent() {
    let f = Fixture::parse("var a = 1;\nvar b = 's';\nvar c = (2);\n");
    let lints = f.run(&one_rule("specify_nonobvious_property_types"));
    assert!(lints.is_empty(), "{lints:?}");
}
