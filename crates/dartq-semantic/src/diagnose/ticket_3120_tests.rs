//! The #3120 pins: the `_resolveType` clause-element-kind check — a
//! clause head that resolves to a non-class/non-mixin element (or misses
//! scope resolution entirely) reports the clause's own code at the
//! written head, never the resolver's `undefined_class`/`not_a_type`
//! (upstream Dart SDK source,
//! pkg/analyzer/lib/src/dart/resolver/resolution_visitor.dart:1169-1241 —
//! `visitExtendsClause`/`visitWithClause`/`visitImplementsClause`/
//! `visitMixinOnClause` all reach the same `_resolveType` walk through
//! `scope_context.dart`'s declaration dispatch).
//!
//! Oracle provenance: `dart analyze` 3.13.7 over the /tmp/uc3120
//! clause-shape battery (`extends`/`implements`/`with`/`on`/`mixin`
//! across missing, prefixed-missing, dynamic, void, enum, extension-type,
//! type-parameter, ambiguous, alias-to-non-class, and resolved-legal
//! heads).
//!
//! Five verdict gaps this file pins:
//!
//! 1. The clause code keys on the CLAUSE, not on the miss's nature: a
//!    bare `Und` in `extends` reports `extends_non_class`, not
//!    `undefined_class` — the upstream `hasErrorReported` gate silences
//!    the resolver once the clause arm lands
//!    (`resolution_visitor.dart:1178-1180`; the `Missing`/`NonType`/
//!    `Prefixed` rows at the same span are suppressed here, matching the
//!    clause-parent suppression `named_type_resolver.dart:673-680` keeps
//!    for every clause EXCEPT `MixinOnClause`).
//! 2. A prefixed `p.Und` in a clause suppresses the `Prefixed` row's
//!    `undefined_class` too — upstream's `_resolveType` early-returns on
//!    `hasErrorReported` before the clause switch, so `extends p.Und`
//!    reports `extends_non_class` at `p.Und`'s whole-written-name span
//!    (the `firstToken = importPrefix ?? name` anchor,
//!    `resolution_visitor.dart:1235-1237`).
//! 3. `on` is additive: `mixin M on Und` keeps the resolver's
//!    `undefined_class` beside `mixin_super_class_constraint_non_
//!    interface` — `MixinOnClause` is the one clause parent
//!    `reportNullOrNonTypeElement` does not suppress
//!    (`named_type_resolver.dart:673-680`'s list omits it, and
//!    `visitMixinOnClause` feeds `onClause` through `_resolveType`,
//!    `scope_context.dart:384-403`).
//! 4. The recovery shapes route by the RECOVERED clause kind: `class A
//!    on Und` reports `extends_non_class` (upstream's `EXPECTED_INSTEAD`
//!    recovery lands the tail in the `ExtendsClause` arm), `mixin M
//!    extends Und` reports `mixin_super_class_constraint_non_interface`
//!    (the `MixinOnClause` arm), and `enum E on Und` reports nothing —
//!    the enum grammar never builds a `MixinOnClause` (`scope_context.
//!    dart:144-167` reaches `with`/`implements` only).
//! 5. A trailing `extends` overflow reports nothing — upstream's
//!    `handleClassExtends` keeps only the first spelled clause (the
//!    recovered `class A extends B, Und` discards the tail heads), so a
//!    `clause_miss` offset no scanned [`SuperEntry`] owns silences with
//!    its row.

use super::test_rig::{
    CORE_PRELUDE, RigImport, assert_silent, report_ops, report_ops_imports, rows,
};

/// The absolute fixture offset of `needle` (the prelude the rig prepends
/// included; the fixtures are ASCII, so byte offset == UTF-16 offset).
fn at(fixture: &str, needle: &str) -> u32 {
    (CORE_PRELUDE.len() + fixture.find(needle).unwrap()) as u32
}

/// One diagnostic's `(code_name, offset, length)` triple — the face the
/// oracle's machine rows compare against. Sorted by (offset, code): the
/// clause code drains at the body-open commit while the parser's own row
/// lands at parse time, so same-offset pairs order emission-side, not
/// oracle-side — the pin compares the SET.
fn code_spans(report: &crate::diagnose::FileReport) -> Vec<(&'static str, u32, u32)> {
    let mut spans: Vec<(&'static str, u32, u32)> = report
        .diagnostics
        .iter()
        .map(|d| (d.code.name, d.range.offset, d.range.length))
        .collect();
    spans.sort_by_key(|(code, offset, _)| (*offset, *code));
    spans
}

// —— 1: the clause code owns the miss, never `undefined_class` ———————————

/// `extends Und` reports `extends_non_class` at the head — the oracle's
/// `ExtendsClause` arm (`resolution_visitor.dart:1215-1219`); the
/// resolver's `undefined_class` fallthrough is suppressed once the clause
/// code lands.
#[test]
fn extends_missing_reports_extends_non_class() {
    let fixture = "class A extends Und {}\n";
    let report = report_ops(fixture);
    assert_eq!(
        code_spans(&report),
        vec![("extends_non_class", at(fixture, "Und"), 3)],
        "rows: {:?}",
        rows(&report)
    );
}

/// `implements Und` reports `implements_non_class` — the
/// `ImplementsClause` arm of the same switch
/// (`resolution_visitor.dart:1221-1222`); no `undefined_class` joins.
#[test]
fn implements_missing_reports_implements_non_class() {
    let fixture = "class A implements Und {}\n";
    let report = report_ops(fixture);
    assert_eq!(
        code_spans(&report),
        vec![("implements_non_class", at(fixture, "Und"), 3)],
        "rows: {:?}",
        rows(&report)
    );
}

/// `with Und` reports `mixin_of_non_class` — the `WithClause` arm
/// (`resolution_visitor.dart:1225-1226`).
#[test]
fn with_missing_reports_mixin_of_non_class() {
    let fixture = "class B {}\nclass A extends B with Und {}\n";
    let report = report_ops(fixture);
    assert_eq!(
        code_spans(&report),
        vec![("mixin_of_non_class", at(fixture, "Und"), 3)],
        "rows: {:?}",
        rows(&report)
    );
}

/// `on Und` keeps BOTH rows: the clause code
/// `mixin_super_class_constraint_non_interface` beside the resolver's
/// `undefined_class` — `MixinOnClause` is the one clause parent the
/// `reportNullOrNonTypeElement` suppression list omits
/// (`named_type_resolver.dart:673-680`).
#[test]
fn mixin_on_missing_reports_constraint_and_undefined_class() {
    let fixture = "mixin M on Und {}\n";
    let report = report_ops(fixture);
    let at_und = at(fixture, "Und");
    assert_eq!(
        code_spans(&report),
        vec![
            ("mixin_super_class_constraint_non_interface", at_und, 3),
            ("undefined_class", at_und, 3),
        ],
        "rows: {:?}",
        rows(&report)
    );
}

// —— 1b: import-prefixed misses —————————————————————————————————————————

// —— 2: the prefixed face suppresses its own row ————————————————————————

/// `extends p.Und` reports `extends_non_class` at `p.Und`'s whole-written-
/// name span (the `importPrefix ?? name` anchor,
/// `resolution_visitor.dart:1235-1237`), and the `Prefixed` row's
/// `undefined_class` silences with it.
#[test]
fn extends_prefixed_missing_reports_extends_non_class() {
    let fixture = "import 'lib.dart' as p;\nclass A extends p.Und {}\n";
    let report = report_ops_imports(
        fixture,
        &[RigImport {
            uri: "lib.dart",
            prefix: Some("p"),
            source: "class Present {}\n",
        }],
    );
    assert_eq!(
        code_spans(&report),
        vec![
            // The prelude's own decls precede the written `import`
            // directive — `directive_after_declaration` @597 is the rig's
            // deterministic layout row, verbatim beside the clause verdict.
            ("directive_after_declaration", at(fixture, "import"), 6),
            ("extends_non_class", at(fixture, "p.Und"), 5),
        ],
        "rows: {:?}",
        rows(&report)
    );
}

/// `implements p.Und` reports `implements_non_class` at the prefixed
/// span — the `ImplementsClause` arm's `firstToken = importPrefix` anchor.
#[test]
fn implements_prefixed_missing_reports_implements_non_class() {
    let fixture = "import 'lib.dart' as p;\nclass A implements p.Und {}\n";
    let report = report_ops_imports(
        fixture,
        &[RigImport {
            uri: "lib.dart",
            prefix: Some("p"),
            source: "class Present {}\n",
        }],
    );
    assert_eq!(
        code_spans(&report),
        vec![
            ("directive_after_declaration", at(fixture, "import"), 6),
            ("implements_non_class", at(fixture, "p.Und"), 5),
        ],
        "rows: {:?}",
        rows(&report)
    );
}

/// `with p.Und` reports `mixin_of_non_class` at the prefixed span.
#[test]
fn with_prefixed_missing_reports_mixin_of_non_class() {
    let fixture = "import 'lib.dart' as p;\nclass B {}\nclass A extends B with p.Und {}\n";
    let report = report_ops_imports(
        fixture,
        &[RigImport {
            uri: "lib.dart",
            prefix: Some("p"),
            source: "class Present {}\n",
        }],
    );
    assert_eq!(
        code_spans(&report),
        vec![
            ("directive_after_declaration", at(fixture, "import"), 6),
            ("mixin_of_non_class", at(fixture, "p.Und"), 5),
        ],
        "rows: {:?}",
        rows(&report)
    );
}

// —— 3: keyword heads — `dynamic`/`void` in the clause ——————————————————

/// `implements dynamic` reports `implements_non_class` at `dynamic` —
/// upstream's `dynamic` resolves to no `InterfaceElement`, so the clause
/// arm fires at the keyword's own span.
#[test]
fn implements_dynamic_reports_implements_non_class() {
    let fixture = "class A implements dynamic {}\n";
    let report = report_ops(fixture);
    assert_eq!(
        code_spans(&report),
        vec![("implements_non_class", at(fixture, "dynamic"), 7)],
        "rows: {:?}",
        rows(&report)
    );
}

/// `extends dynamic` reports `extends_non_class` at `dynamic`.
#[test]
fn extends_dynamic_reports_extends_non_class() {
    let fixture = "class A extends dynamic {}\n";
    let report = report_ops(fixture);
    assert_eq!(
        code_spans(&report),
        vec![("extends_non_class", at(fixture, "dynamic"), 7)],
        "rows: {:?}",
        rows(&report)
    );
}

/// `on dynamic` reports `mixin_super_class_constraint_non_interface`.
#[test]
fn on_dynamic_reports_constraint_non_interface() {
    let fixture = "mixin M on dynamic {}\n";
    let report = report_ops(fixture);
    assert_eq!(
        code_spans(&report),
        vec![(
            "mixin_super_class_constraint_non_interface",
            at(fixture, "dynamic"),
            7
        )],
        "rows: {:?}",
        rows(&report)
    );
}

/// `extends void` reports `extends_non_class` at `void` — the parser's
/// `expected_type_name` row fires separately (the recovered `void` keeps
/// a real type position upstream, `resolution_visitor.dart`'s
/// `_resolveType` reads the `Void` type's element — null — and reports
/// the clause arm).
#[test]
fn extends_void_reports_extends_non_class() {
    let fixture = "class A extends void {}\n";
    let report = report_ops(fixture);
    let at_void = at(fixture, "void");
    assert_eq!(
        code_spans(&report),
        vec![
            ("expected_type_name", at_void, 4),
            ("extends_non_class", at_void, 4),
        ],
        "rows: {:?}",
        rows(&report)
    );
}

// —— 4: resolved but illegal kinds ——————————————————————————————————————

/// `extends Enum` reports `extends_non_class` — an `EnumElement` is not a
/// `ClassElement`/`MixinElement`, so the clause arm fires
/// (`resolution_visitor.dart:1194-1202`); the resolver's `not_a_type` row
/// (or `undefined_class` on the recovered shape) stays suppressed.
#[test]
fn extends_enum_reports_extends_non_class() {
    let fixture = "enum E { v }\nclass A extends E {}\n";
    let report = report_ops(fixture);
    assert_eq!(
        code_spans(&report),
        vec![
            // The enum constant's own induced instance getter collides
            // with the constant's static field — a deterministic
            // `conflicting_static_and_instance` row (#3124's in-flight
            // face), verbatim, beside the clause arm's own verdict.
            ("conflicting_static_and_instance", at(fixture, "v }"), 1),
            ("extends_non_class", at(fixture, "E {}"), 1),
        ],
        "rows: {:?}",
        rows(&report)
    );
}

/// `implements Enum` reports `implements_non_class`.
#[test]
fn implements_enum_reports_implements_non_class() {
    let fixture = "enum E { v }\nclass A implements E {}\n";
    let report = report_ops(fixture);
    assert_eq!(
        code_spans(&report),
        vec![
            ("conflicting_static_and_instance", at(fixture, "v }"), 1),
            ("implements_non_class", at(fixture, "E {}"), 1),
        ],
        "rows: {:?}",
        rows(&report)
    );
}

/// `with Enum` reports `mixin_of_non_class`.
#[test]
fn with_enum_reports_mixin_of_non_class() {
    let fixture = "enum E { v }\nclass B {}\nclass A extends B with E {}\n";
    let report = report_ops(fixture);
    assert_eq!(
        code_spans(&report),
        vec![
            ("conflicting_static_and_instance", at(fixture, "v }"), 1),
            ("mixin_of_non_class", at(fixture, "E {}"), 1),
        ],
        "rows: {:?}",
        rows(&report)
    );
}

/// `extends TypeParam` reports `extends_non_class` — a `TypeParameterElement`
/// is not a `ClassElement`.
#[test]
fn extends_type_param_reports_extends_non_class() {
    let fixture = "class A<T> extends T {}\n";
    let report = report_ops(fixture);
    assert_eq!(
        code_spans(&report),
        vec![("extends_non_class", at(fixture, "T {}"), 1)],
        "rows: {:?}",
        rows(&report)
    );
}

/// `implements TypeParam` reports `implements_non_class`.
#[test]
fn implements_type_param_reports_implements_non_class() {
    let fixture = "class A<T> implements T {}\n";
    let report = report_ops(fixture);
    assert_eq!(
        code_spans(&report),
        vec![("implements_non_class", at(fixture, "T {}"), 1)],
        "rows: {:?}",
        rows(&report)
    );
}

/// `extends Fn` where `typedef Fn = void Function()` reports
/// `extends_non_class` — a function-type alias's element is a
/// `TypeAliasElement`, not a `ClassElement`.
#[test]
fn extends_function_typedef_reports_extends_non_class() {
    let fixture = "typedef Fn = void Function();\nclass A extends Fn {}\n";
    let report = report_ops(fixture);
    assert_eq!(
        code_spans(&report),
        vec![("extends_non_class", at(fixture, "Fn {}"), 2)],
        "rows: {:?}",
        rows(&report)
    );
}

/// `implements Fn` where `typedef Fn = void Function()` reports
/// `implements_non_class`.
#[test]
fn implements_function_typedef_reports_implements_non_class() {
    let fixture = "typedef Fn = void Function();\nclass A implements Fn {}\n";
    let report = report_ops(fixture);
    assert_eq!(
        code_spans(&report),
        vec![("implements_non_class", at(fixture, "Fn {}"), 2)],
        "rows: {:?}",
        rows(&report)
    );
}

/// `extends Mixin` reports `extends_non_class` — a `MixinElement` is
/// legal only in `implements`/`with`/`on`, never `extends`
/// (`resolution_visitor.dart:1197-1202`).
#[test]
fn extends_mixin_reports_extends_non_class() {
    let fixture = "mixin M {}\nclass A extends M {}\n";
    let report = report_ops(fixture);
    assert_eq!(
        code_spans(&report),
        vec![("extends_non_class", at(fixture, "extends M") + 8, 1)],
        "rows: {:?}",
        rows(&report)
    );
}

/// `implements Mixin` resolves clean — the `ImplementsClause` arm accepts
/// a `MixinElement`.
#[test]
fn implements_mixin_is_silent() {
    let report = report_ops("mixin M {}\nclass A implements M {}\n");
    assert_silent(&report);
}

/// `with Mixin` resolves clean — the `WithClause` arm accepts a
/// `MixinElement`.
#[test]
fn with_mixin_is_silent() {
    let report = report_ops("mixin M {}\nclass B {}\nclass A extends B with M {}\n");
    assert_silent(&report);
}

/// `on Mixin` resolves clean — the `MixinOnClause` arm accepts a
/// `MixinElement`.
#[test]
fn on_mixin_is_silent() {
    let report = report_ops("mixin M {}\nmixin N on M {}\n");
    assert_silent(&report);
}

/// `on Class` resolves clean — the `MixinOnClause` arm accepts a
/// `ClassElement`.
#[test]
fn on_class_is_silent() {
    let report = report_ops("class C {}\nmixin M on C {}\n");
    assert_silent(&report);
}

/// `extends Class` resolves clean — the `ExtendsClause` arm accepts a
/// `ClassElement`.
#[test]
fn extends_class_is_silent() {
    let report = report_ops("class B {}\nclass A extends B {}\n");
    assert_silent(&report);
}

/// `implements Class` resolves clean — the `ImplementsClause` arm accepts
/// a `ClassElement`.
#[test]
fn implements_class_is_silent() {
    let report = report_ops("class B {}\nclass A implements B {}\n");
    assert_silent(&report);
}

// —— 5: recovery shapes ————————————————————————————————————————————————

/// `class A on Und` is the upstream `EXPECTED_INSTEAD` recovery — the
/// `on` tail lands in the `ExtendsClause` arm, so `Und` reports
/// `extends_non_class`, never `mixin_super_class_constraint_non_interface`
/// or the resolver's `undefined_class`.
#[test]
fn class_on_missing_reports_extends_non_class() {
    let fixture = "class A on Und {}\n";
    let report = report_ops(fixture);
    assert_eq!(
        code_spans(&report),
        vec![
            ("expected_instead", at(fixture, "on"), 2),
            ("extends_non_class", at(fixture, "Und"), 3),
        ],
        "rows: {:?}",
        rows(&report)
    );
}

/// `mixin M extends Und` is the same recovery: the `extends` tail lands
/// in the `MixinOnClause` arm, so `Und` reports
/// `mixin_super_class_constraint_non_interface` beside the resolver's
/// `undefined_class` (the additive face).
#[test]
fn mixin_extends_missing_reports_constraint_and_undefined_class() {
    let fixture = "mixin M extends Und {}\n";
    let report = report_ops(fixture);
    let at_und = at(fixture, "Und");
    assert_eq!(
        code_spans(&report),
        vec![
            ("expected_instead", at(fixture, "extends"), 7),
            ("mixin_super_class_constraint_non_interface", at_und, 3),
            ("undefined_class", at_und, 3),
        ],
        "rows: {:?}",
        rows(&report)
    );
}

/// `enum E on Und` reports nothing beyond the parser's own tokens —
/// the enum grammar never builds a `MixinOnClause`, so `_resolveType`
/// never walks the name (`scope_context.dart:144-167`'s clause dispatch
/// covers `with`/`implements` only).
#[test]
fn enum_on_missing_reports_nothing_beyond_parser() {
    let fixture = "enum E on Und {}\n";
    let report = report_ops(fixture);
    assert_eq!(
        code_spans(&report),
        vec![
            ("enum_without_constants", at(fixture, "E"), 1),
            ("unexpected_tokens", at(fixture, "on Und"), 6),
        ],
        "rows: {:?}",
        rows(&report)
    );
}

/// `enum E with Und` — the enum's `with` clause feeds `_resolveType` the
/// same as a class's (`scope_context.dart:144-167`'s
/// `node.withClause?.accept`); `Und` reports `mixin_of_non_class`.
#[test]
fn enum_with_missing_reports_mixin_of_non_class() {
    let fixture = "enum E with Und { v }\n";
    let report = report_ops(fixture);
    assert_eq!(
        code_spans(&report),
        vec![
            ("mixin_of_non_class", at(fixture, "Und"), 3),
            ("conflicting_static_and_instance", at(fixture, "v }"), 1),
        ],
        "rows: {:?}",
        rows(&report)
    );
}

/// `enum E implements Und` reports `implements_non_class` — the enum's
/// `implements` clause feeds `_resolveType` (`scope_context.dart:156`).
#[test]
fn enum_implements_missing_reports_implements_non_class() {
    let fixture = "enum E implements Und { v }\n";
    let report = report_ops(fixture);
    assert_eq!(
        code_spans(&report),
        vec![
            ("implements_non_class", at(fixture, "Und"), 3),
            ("conflicting_static_and_instance", at(fixture, "v }"), 1),
        ],
        "rows: {:?}",
        rows(&report)
    );
}

/// `class C = Und with M` reports `mixin_with_non_class_superclass` at
/// the `=` head — `visitClassTypeAlias` feeds the superclass through
/// `_resolveType` with `clause: null`, and the `ClassTypeAlias`
/// declaration keys `mixinWithNonClassSuperclass`
/// (`resolution_visitor.dart:1211-1213`).
#[test]
fn named_mixin_application_missing_superclass_reports_mixin_with_non_class_superclass() {
    let fixture = "mixin M {}\nclass C = Und with M;\n";
    let report = report_ops(fixture);
    assert_eq!(
        code_spans(&report),
        vec![("mixin_with_non_class_superclass", at(fixture, "Und"), 3)],
        "rows: {:?}",
        rows(&report)
    );
}

/// `mixin M with Und` — upstream's mixin grammar never builds a
/// `WithClause` (`visitMixinDeclaration` visits `on`/`implements` only,
/// `scope_context.dart:384-403`), so `Und` reports nothing; the parser's
/// `mixin_with_clause` row is the only diagnostic.
#[test]
fn mixin_with_clause_is_silent_beyond_parser_row() {
    let fixture = "mixin M with Und {}\n";
    let report = report_ops(fixture);
    assert_eq!(
        code_spans(&report),
        vec![("mixin_with_clause", at(fixture, "with"), 4)],
        "rows: {:?}",
        rows(&report)
    );
}

/// `class A extends B, Und` — upstream's `handleClassExtends` keeps only
/// the first spelled clause (the ordering-recovery tail heads are
/// discarded), so `Und` carries no clause face and reports nothing; the
/// `multiple_extends_clauses` parser row is the only diagnostic.
#[test]
fn multiple_extends_tail_is_silent() {
    let fixture = "class B {}\nclass A extends B, Und {}\n";
    let report = report_ops(fixture);
    assert_eq!(
        code_spans(&report),
        vec![("multiple_extends_clauses", at(fixture, ","), 1)],
        "rows: {:?}",
        rows(&report)
    );
}

// —— 6: the `on` additive face ——————————————————————————————————————————

/// `mixin M on x` where `x` is a top-level variable keeps BOTH rows:
/// `mixin_super_class_constraint_non_interface` beside the resolver's
/// `not_a_type` — `MixinOnClause` is the one clause parent
/// `reportNullOrNonTypeElement` does not suppress
/// (`named_type_resolver.dart:673-680`), so the resolved-non-type face
/// reports the resolver row beside the clause code.
#[test]
fn mixin_on_nonclass_reports_constraint_and_not_a_type() {
    let fixture = "int x = 0;\nmixin M on x {}\n";
    let report = report_ops(fixture);
    let at_x = at(fixture, "x {}");
    assert_eq!(
        code_spans(&report),
        vec![
            ("mixin_super_class_constraint_non_interface", at_x, 1),
            ("not_a_type", at_x, 1),
        ],
        "rows: {:?}",
        rows(&report)
    );
}

/// `mixin M on E` where `E` is an enum reports
/// `mixin_super_class_constraint_non_interface` — the resolved element
/// is not a `ClassElement`/`MixinElement`, so the clause arm fires.
#[test]
fn mixin_on_enum_reports_constraint_non_interface() {
    let fixture = "enum E { v }\nmixin M on E {}\n";
    let report = report_ops(fixture);
    assert_eq!(
        code_spans(&report),
        vec![
            ("conflicting_static_and_instance", at(fixture, "v }"), 1),
            (
                "mixin_super_class_constraint_non_interface",
                at(fixture, "E {}"),
                1
            ),
        ],
        "rows: {:?}",
        rows(&report)
    );
}

// —— 7: resolved legal faces stay silent ————————————————————————————————

/// `extends MixinClass` resolves clean — a `mixin class` is a
/// `ClassElement` first, legal in `extends`.
#[test]
fn extends_mixin_class_is_silent() {
    let report = report_ops("mixin class M {}\nclass A extends M {}\n");
    assert_silent(&report);
}

/// `with MixinClass` resolves clean — a `mixin class` answers
/// `ClassElement` first.
#[test]
fn with_mixin_class_is_silent() {
    let report = report_ops("mixin class M {}\nclass B {}\nclass A extends B with M {}\n");
    assert_silent(&report);
}

/// `mixin N implements M` is silent — `MixinElement` is legal in
/// `implements`.
#[test]
fn mixin_implements_mixin_is_silent() {
    let report = report_ops("mixin M {}\nmixin N implements M {}\n");
    assert_silent(&report);
}

/// `mixin M implements C` is silent — a mixin can `implement` a
/// `ClassElement`.
#[test]
fn mixin_implements_class_is_silent() {
    let report = report_ops("class C {}\nmixin M implements C {}\n");
    assert_silent(&report);
}

// —— 8: the typedef RHS is not the clause face ——————————————————————————

/// `class A extends T` where `typedef T = Und` reports `extends_non_
/// class` at `T` (the clause face's own head — the alias's element is a
/// `TypeAliasElement`, not a `ClassElement`), and the RHS `Und` reports
/// its own `undefined_class` at the typedef's RHS.
#[test]
fn typedef_rhs_missing_reports_clause_and_rhs() {
    let fixture = "typedef T = Und;\nclass A extends T {}\n";
    let report = report_ops(fixture);
    assert_eq!(
        code_spans(&report),
        vec![
            ("undefined_class", at(fixture, "Und"), 3),
            ("extends_non_class", at(fixture, "T {}"), 1),
        ],
        "rows: {:?}",
        rows(&report)
    );
}

// —— 9: `void` recovery artifact ————————————————————————————————————————

/// `implements void` reports `implements_non_class` at `void` plus the
/// parser's `expected_type_name` — the recovered `void` keeps a real type
/// position upstream, and the clause arm fires at the keyword's own span
/// (never at the `{`/`}` artifact the `parse_type_not_void` re-fire
/// would anchor).
#[test]
fn implements_void_reports_implements_non_class() {
    let fixture = "class A implements void {}\n";
    let report = report_ops(fixture);
    let at_void = at(fixture, "void");
    assert_eq!(
        code_spans(&report),
        vec![
            ("expected_type_name", at_void, 4),
            ("implements_non_class", at_void, 4),
        ],
        "rows: {:?}",
        rows(&report)
    );
}

/// `on void` reports `mixin_super_class_constraint_non_interface` plus
/// `expected_type_name` — the same `void`-in-clause recovery.
#[test]
fn on_void_reports_constraint_non_interface() {
    let fixture = "mixin M on void {}\n";
    let report = report_ops(fixture);
    let at_void = at(fixture, "void");
    assert_eq!(
        code_spans(&report),
        vec![
            ("expected_type_name", at_void, 4),
            ("mixin_super_class_constraint_non_interface", at_void, 4),
        ],
        "rows: {:?}",
        rows(&report)
    );
}
