
//! The dead/unused-family differential pins: the corpus `dart analyze`
//! `(severity, code, file, line:col)` burn-down, rooted in four oracle
//! mechanisms each fixture reproduces end-to-end.
//!
//! - `x is T` / `x is! T` with an unresolvable `T`: the oracle's flow
//!   BAILS — `type_test_with_undefined_name` fires on the spelling and
//!   neither branch reports dead (#2450 counter-finding; the probe pair
//!   `p12_isdead`). The pre-#2450 always-true fold over-reported the
//!   losing branch; its removal is pinned below.
//! - `unused_field`'s `unresolvedReadMembers` fallback
//!   (unused_local_elements_verifier.dart:1030-1055): a read-shaped
//!   member access that bound no element (`f._x` on an
//!   invalid-typed receiver) still spares the field by spelling.
//! - `_recordEnumValuesUse` (:442-455): reading an enum's `values`
//!   getter credits every constant's getter at once.
//! - The extension credit's receiver-validity gate: an access through an
//!   unresolvable receiver type (the broken-import corpus's dominant
//!   shape — `m.g` on an undefined `Missing`) binds nothing and credits
//!   nothing, so the accessed member reports; an access through a VALID
//!   receiver applies an invalid-`on` extension vacuously (the subtype
//!   check against the InvalidType extended type passes) and spares it.
//!   The credit is judged at the resolution side and the declaration
//!   walk trusts it as recorded.

use super::*;
use crate::collector::collect;
use crate::resolved_ops::{LibraryTypes, ResolvedTypeOps};
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::{LibraryClassModifiers, LibraryGraphBuilder};
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

fn scan(source: &str, file: FileId) -> (dartq_scanner::ScanResult, ParserFeatures, FeatureSet) {
    let scanned = dartq_scanner::scan(
        source,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        file,
    );
    assert!(
        scanned.diagnostics.is_empty(),
        "scanner diagnostics: {source:?}"
    );
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    let features = ParserFeatures::from_feature_set(set);
    (scanned, features, set)
}

/// Diagnoses `source` as the sole file of a library with the resolved
/// `TypeOps` face mounted — the engine path the `x is Missing` tested
/// face resolves through (the name-level ops cannot see `InvalidType`).
fn report_ops(source: &str) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let (scanned, features, set) = scan(source, file);
    let elements = collect(source, &scanned.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
    builder.unit(lib, file);
    let mut index = ElementIndex::default();
    index.insert(file, elements.clone());
    for (name, decl) in elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib, name, file);
    }
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let types = LibraryTypes::build(&graph, &namespaces, &index, &strings, lib);
    let ops = ResolvedTypeOps::new(&types, &strings);
    let fold = fold_unit_with_ops(
        source,
        &scanned.tokens,
        features,
        &strings,
        file,
        lib,
        &graph,
        &namespaces,
        &index,
        StrictModes::default(),
        &LibraryClassModifiers::default(),
        set.is_enabled(dartq_language::Feature::InferenceUpdate2),
        Some(&ops),
    );
    let mut report = fold.report;
    report.diagnostics = merge_source_ordered(fold.parse_diagnostics, report.diagnostics);
    report
}

/// The single-file rig without a resolved ops face — the unused-*
/// verdicts are name-level by construction.
fn report(source: &str) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let (scanned, features, set) = scan(source, file);
    let elements = collect(source, &scanned.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
    builder.unit(lib, file);
    let mut index = ElementIndex::default();
    index.insert(file, elements.clone());
    for (name, decl) in elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib, name, file);
    }
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let fold = fold_unit(
        source,
        &scanned.tokens,
        features,
        &strings,
        file,
        lib,
        &graph,
        &namespaces,
        &index,
        StrictModes::default(),
        &LibraryClassModifiers::default(),
        set.is_enabled(dartq_language::Feature::InferenceUpdate2),
    );
    let mut report = fold.report;
    report.diagnostics = merge_source_ordered(fold.parse_diagnostics, report.diagnostics);
    report
}

fn code_names(report: &FileReport) -> Vec<&'static str> {
    report.diagnostics.iter().map(|d| d.code.name).collect()
}

fn rows(report: &FileReport, code: &str) -> Vec<(u32, u32)> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| (d.range.offset, d.range.length))
        .collect()
}

// —— dead_code: the InvalidType checked type ————————————————————————

// 2026-10-22 re-probe (Dart 3.13.1, `dart analyze --format=machine`): the
// both-arms posture pinned here was wrong — the live oracle deads the
// test-false arm of every undefined-tested is-check (int/dynamic/invalid
// operand, both polarities). The mechanism is the reference path's
// `factor(previous, InvalidType)` = `Never` (`subtype.dart:274` makes any
// relation with an invalid type a subtype), so `ifFalseIsUnreachable`
// fires inside `tryPromoteForTypeCheck` (flow_analysis.dart:4156-4160,
// :4188-4190). The pre-#2450 constant-literal fold had the dead arm
// right but the fold face wrong; these pins carry the split's posture.

/// `x is Missing` reports the undefined-name code and deads the else
/// arm: the dead row anchors inside the else block.
#[test]
fn is_test_with_undefined_name_deads_the_else() {
    let source = "void f(int x) {\n\
                      \x20 if (x is Missing) {\n\
                      \x20   x;\n\
                      \x20 } else {\n\
                      \x20   x;\n\
                      \x20 }\n\
                      }";
    let report = report_ops(source);
    let dead = rows(&report, "dead_code");
    let else_start = source.find("else").unwrap() as u32;
    assert_eq!(
        dead.len(),
        1,
        "exactly the else arm carries the dead row: {:?}",
        report.diagnostics
    );
    assert!(
        dead[0].0 > else_start,
        "the dead row anchors inside the else arm: {:?}",
        report.diagnostics
    );
    assert!(
        code_names(&report)
            .iter()
            .any(|name| *name == "type_test_with_undefined_name"),
        "the undefined spelling reports its own code: {:?}",
        report.diagnostics
    );
}

/// `x is! Missing` — the `is!` polarity of the same split: the test
/// answers `true`, so the inverted then arm is the dead one.
#[test]
fn is_not_test_with_undefined_name_deads_the_then() {
    let source = "void f(int x) {\n\
                      \x20 if (x is! Missing) {\n\
                      \x20   x;\n\
                      \x20 } else {\n\
                      \x20   x;\n\
                      \x20 }\n\
                      }";
    let report = report_ops(source);
    let dead = rows(&report, "dead_code");
    let test_start = source.find("is!").unwrap() as u32;
    let else_start = source.find("else").unwrap() as u32;
    assert_eq!(
        dead.len(),
        1,
        "exactly the then arm carries the dead row: {:?}",
        report.diagnostics
    );
    assert!(
        dead[0].0 > test_start && dead[0].0 < else_start,
        "the dead row anchors inside the then arm: {:?}",
        report.diagnostics
    );
    assert!(
        code_names(&report)
            .iter()
            .any(|name| *name == "type_test_with_undefined_name"),
        "the undefined spelling reports its own code: {:?}",
        report.diagnostics
    );
}

/// A collection-if's dead else-arm reports its own extent — the arm
/// element's start to its end, nothing after (official
/// `handle_ifElement_elseEnd` → `flowEnd(ifFalse)`,
/// type_analyzer.dart:1168-1171; oracle-probed on Dart 3.13.1: the
/// `[if (x is Missing) 'a' else 'b', 'c']` world carries exactly
/// `'b'`'s three-char row, and the element's join restores
/// reachability so no later element reports).
#[test]
fn collection_if_dead_else_arm_rows_its_own_extent() {
    let source = concat!(
        "void f(int x) {\n",
        "  var l = <String>[\n",
        "    if (x is Missing) 'a' else 'b',\n",
        "    'c',\n",
        "  ];\n",
        "  print(l);\n",
        "}\n",
    );
    let report = report_ops(source);
    let dead = rows(&report, "dead_code");
    let b = source.find("'b'").unwrap() as u32;
    assert_eq!(
        dead.len(),
        1,
        "exactly the else element carries the dead row: {:?}",
        report.diagnostics
    );
    assert_eq!(
        dead[0],
        (b, 3),
        "the dead row spans the else element exactly: {:?}",
        report.diagnostics
    );
}

/// A resolvable narrowing test keeps both arms live — `factor(Parent,
/// Child)` is `Parent`, not `Never`, so the else stays reachable (the
/// `Invalid` verdict must not leak into the resolved-name case; an
/// already-satisfied test deads the else in the oracle too — the
/// `unnecessary_type_check` pair — so the control narrows instead).
#[test]
fn is_test_with_resolved_name_keeps_both_arms() {
    let source = "class Parent {}\n\
                      class Child extends Parent {}\n\
                      void f(Parent x) {\n\
                      \x20 if (x is Child) {\n\
                      \x20   x;\n\
                      \x20 } else {\n\
                      \x20   x;\n\
                      \x20 }\n\
                      }";
    let report = report_ops(source);
    assert!(
        rows(&report, "dead_code").is_empty(),
        "no dead arm for a narrowing is-test: {:?}",
        report.diagnostics
    );
}

// —— unused_field: unresolvedReadMembers + enum `values` ————————————

/// `f._x` on an invalid-typed receiver spares `_x` by spelling: the
/// read bound no element, so the oracle files it under
/// `unresolvedReadMembers` — the name alone suffices for the field
/// verdict (`_isReadMember`'s second arm,
/// unused_local_elements_verifier.dart:1049-1050).
#[test]
fn unresolved_receiver_read_spares_the_field() {
    let source = "class C {\n\
                      \x20 int _x = 0;\n\
                      }\n\
                      void f(Missing m) {\n\
                      \x20 print(m._x);\n\
                      }";
    let report = report(source);
    assert!(
        rows(&report, "unused_field").is_empty(),
        "the unresolved read spares `_x`: {:?}",
        report.diagnostics
    );
}

/// The oracle's `_recordEnumValuesUse`: reading an enum's `values`
/// getter credits every constant — `E.values` on a private enum
/// leaves no `unused_field` rows even when no constant is read by
/// name.
#[test]
fn enum_values_read_spares_every_constant() {
    let source = "enum _E { a, b, c }\n\
                      void f() {\n\
                      \x20 for (var v in _E.values) {\n\
                      \x20   v;\n\
                      \x20 }\n\
                      }";
    let report = report(source);
    assert!(
        rows(&report, "unused_field").is_empty(),
        "`_E.values` spares every constant: {:?}",
        report.diagnostics
    );
}

/// A private enum constant never read by name or through `values`
/// still reports — the credit is scoped to the `.values` access, not
/// the enum's whole face.
#[test]
fn unread_enum_constant_still_reports() {
    let source = "enum _E { a, b }\n\
                      void f() {\n\
                      \x20 _E.a;\n\
                      }";
    let report = report(source);
    let bare_b = source.find("b }").unwrap() as u32;
    assert!(
        rows(&report, "unused_field")
            .iter()
            .any(|&(offset, _)| offset == bare_b),
        "`b` was never read: {:?}",
        report.diagnostics
    );
}

// —— unused_element: the extension credit's receiver-validity gate ————
//
// The oracle truth table (probed against `dart analyze` 3.13.1, a
// self-package `package_config.json` mapping the probing directory —
// the corpus's broken-import world):
//
// - an access through a VALID receiver spares a member of an
//   extension whose `on` type is itself invalid — the subtype check
//   against the InvalidType extended type passes vacuously, the member
//   lookup inside the extension succeeds, and `usedElements.members`
//   gains it (`c.g` on a valid `C` with `extension _E on Missing`
//   emits neither `undefined_getter` nor `unused_element`; dropping
//   the extension brings the getter miss back);
// - an access through an INVALID receiver (or no access at all)
//   credits nothing — the lookup dies on the unresolvable receiver
//   before the extension probe — so the member reports;
// - a bare identifier read inside a SIBLING extension is an
//   implicit-`this` read on the sibling's `on` type: no extension
//   override is created and the member stays uncredited.
//
// The credit set is therefore judged at the resolution side (receiver
// validity), and the declaration-site walk trusts it as recorded.

/// The corpus's dominant `unused_element` family: an invalid receiver
/// (`m` typed by an undefined class) credits nothing, so an accessed
/// member of an extension whose `on` type also fails to resolve still
/// reports.
#[test]
fn extension_member_on_invalid_receiver_still_reports() {
    let source = "extension _E on Missing {\n\
                      \x20 int get g => 0;\n\
                      }\n\
                      void f(Missing m) {\n\
                      \x20 m.g;\n\
                      }";
    let report = report_ops(source);
    assert!(
        rows(&report, "unused_element")
            .iter()
            .any(|&(offset, _)| { offset == source.find("get g").unwrap() as u32 + 4 }),
        "the member accessed through an invalid receiver reports: {:?}",
        report.diagnostics
    );
}

/// Without any access the member reports regardless of the `on`
/// clause — the unaccessed half of the corpus family.
#[test]
fn extension_member_reports_without_access() {
    let source = "class C {}\n\
                      extension _E on Missing {\n\
                      \x20 int get g => 0;\n\
                      }";
    let report = report(source);
    assert!(
        rows(&report, "unused_element")
            .iter()
            .any(|&(offset, _)| { offset == source.find("get g").unwrap() as u32 + 4 }),
        "the unaccessed member of `extension _E on Missing` reports: {:?}",
        report.diagnostics
    );
}

/// A public member of an UNNAMED extension is checked like a private
/// declaration (the extension itself has no name to be exported by),
/// so the corpus's `extension on Element?` getter family reports when
/// nothing applies to it.
#[test]
fn unnamed_extension_member_reports_when_never_applied() {
    let source = "extension on Missing {\n\
                      \x20 bool get isFoo => true;\n\
                      }";
    let report = report(source);
    assert!(
        rows(&report, "unused_element")
            .iter()
            .any(|&(offset, _)| { offset == source.find("get isFoo").unwrap() as u32 + 4 }),
        "the unnamed extension's member reports: {:?}",
        report.diagnostics
    );
}

/// The ffi_verifier shape end to end: the extension's `on` type and
/// the accessing receiver's declared type are BOTH undefined, so the
/// access binds nothing and the member reports.
#[test]
fn unnamed_extension_member_reports_for_invalid_receiver_access() {
    let source = "void f(Missing m) {\n\
                      \x20 m.isFoo;\n\
                      }\n\
                      extension on Missing {\n\
                      \x20 bool get isFoo => true;\n\
                      }";
    let report = report_ops(source);
    assert!(
        rows(&report, "unused_element")
            .iter()
            .any(|&(offset, _)| { offset == source.find("get isFoo").unwrap() as u32 + 4 }),
        "the member accessed through the invalid receiver reports: {:?}",
        report.diagnostics
    );
}

/// `assert(isArray)` inside a sibling extension is an implicit-`this`
/// read on the SIBLING's `on` type: no extension override engages, the
/// read credits nothing, and both the read member and the reading
/// method report.
#[test]
fn unnamed_extension_bare_sibling_read_credits_nothing() {
    let source = "extension on A {\n\
                      \x20 bool get isArray => true;\n\
                      }\n\
                      extension on B {\n\
                      \x20 void m() {\n\
                      \x20\x20 assert(isArray);\n\
                      \x20 }\n\
                      }\n\
                      class A {}\n\
                      class B {}";
    let report = report(source);
    let getter_at = source.find("get isArray").unwrap() as u32 + 4;
    let method_at = source.find("void m").unwrap() as u32 + 5;
    let unused = rows(&report, "unused_element");
    assert!(
        unused.iter().any(|&(offset, _)| offset == getter_at),
        "the cross-extension bare read spares nothing: {:?}",
        report.diagnostics
    );
    assert!(
        unused.iter().any(|&(offset, _)| offset == method_at),
        "the never-applied sibling method reports: {:?}",
        report.diagnostics
    );
}

/// The `visitWithOverride` corpus shape: a generic unnamed extension
/// (`extension<T extends C> on T`) whose parameter-headed `on` type
/// can never resolve — its method reports when never applied.
#[test]
fn unnamed_generic_extension_method_reports() {
    let source = "class C {}\n\
                      extension<T extends C> on T {\n\
                      \x20 void visitWithOverride() {}\n\
                      }";
    let report = report(source);
    assert!(
        rows(&report, "unused_element")
            .iter()
            .any(|&(offset, _)| { offset == source.find("visitWithOverride").unwrap() as u32 }),
        "the generic unnamed extension's method reports: {:?}",
        report.diagnostics
    );
}

/// A named-private extension over a resolving generic `on` head
/// (`Map<K, V>` with local arguments) that is never applied: the
/// member reports despite the resolvable head.
#[test]
fn named_private_extension_generic_on_reports_when_never_applied() {
    let source = "class K {}\n\
                      class V {}\n\
                      extension _M on Map<K, V> {\n\
                      \x20 int get entry => 1;\n\
                      }";
    let report = report(source);
    assert!(
        rows(&report, "unused_element")
            .iter()
            .any(|&(offset, _)| { offset == source.find("get entry").unwrap() as u32 + 4 }),
        "the never-applied member of the generic-`on` extension reports: {:?}",
        report.diagnostics
    );
}

/// The resolved `on` control: `extension _E on C` applies to `c.g`,
/// so the member spares.
#[test]
fn extension_on_resolved_type_credits_members() {
    let source = "class C {}\n\
                      extension _E on C {\n\
                      \x20 int get g => 0;\n\
                      }\n\
                      void f(C c) {\n\
                      \x20 c.g;\n\
                      }";
    let report = report(source);
    assert!(
        rows(&report, "unused_element").is_empty(),
        "the member of `extension _E on C` spares: {:?}",
        report.diagnostics
    );
}

/// The per-level report+reset of a dead right operand
/// (`checkUnreachableNode(rightOperand)` marks the first-dead node at the
/// operand at `rightBegin`; each level's `flowEnd(rightOperand)` then
/// re-anchors to its own operator, reports, and resets —
/// binary_expression_resolver.dart:274-316, dead_code_verifier.dart:238-339).
/// An `is!`-chain: every level whose rhs ENTERS dead reports its own
/// `[operator, rightOperand.end]` row, the inner live `||` rhs stays
/// silent, and the whole then-arm row follows (oracle rows 2:21+22,
/// 3:21+40, 4:39+22, 5:22+16).
#[test]
fn dead_rhs_chain_reports_per_level_with_reset() {
    let source = "void f(Object? rules) {\n  if (rules is! Aaa &&\n      rules is! Bbb &&\n      (rules is! Ccc || rules is Ddd) &&\n      rules != null) {\n    rules;\n  }\n}\n";
    let report = report_ops(source);
    let rows = rows(&report, "dead_code");
    let expected = [
        (source.find("&&\n      rules is! Bbb").unwrap() as u32, 22),
        (source.find("&&\n      (rules is! Ccc").unwrap() as u32, 40),
        (source.find("&&\n      rules != null").unwrap() as u32, 22),
        (source.find("{\n    rules;").unwrap() as u32, 16),
    ];
    assert_eq!(rows, expected, "{:?}", code_names(&report));
}

/// A nowhere-resolving bare-head call's result is the oracle's
/// `InvalidType` — `_inferLocalVariable` types the declared slot with it,
/// so every `is`-test on the local succeeds vacuously (subtype.dart's 0c
/// arm) and the failing arm is dead code. The resolved back-end's
/// `call_head_unresolved` answers the head (the flow's unresolved-call
/// face), and the corpus's broken-import files carry this shape by the
/// dozen (`var t = pop(); if (t is _CascadeBuilder) … else …` —
/// ast_builder.dart:214; oracle-probed: DEAD_CODE spans the else block,
/// no `unnecessary_type_check` rides along).
#[test]
fn unresolved_call_initializer_deads_the_else_arm() {
    let source = "class Cb {}\nvoid f() {\n  var t = nowheresfn();\n  if (t is Cb) {\n    t.hashCode;\n  } else {\n    print(2);\n  }\n}\n";
    let report = report_ops(source);
    let rows = rows(&report, "dead_code");
    let start = source.find("print(2)").unwrap() as u32 - 6;
    assert_eq!(rows, vec![(start, 19)], "{:?}", code_names(&report));
}

/// An always-true conditional (`c ? a : e` whose test a promotion already
/// decides) rows its else operand whole — the official
/// `flowEnd(elseExpression)` re-anchors past any poll an inner node
/// opened (the as-cast's type poll): the first dead node is the arm
/// expression itself (type_analyzer.dart:2616; oracle-probed:
/// `typeName is Q ? typeName.name : typeName as Q` inside the promotion
/// reports DEAD_CODE at exactly the `typeName as Q` operand and
/// `unnecessary_cast` beside it).
#[test]
fn always_true_conditional_rows_the_else_operand_whole() {
    let source = "class Q { int name = 1; }\nvoid h(dynamic typeName) {\n  if (typeName is Q) {\n    var x = typeName is Q ? typeName.name : typeName as Q;\n    print(x);\n  }\n}\n";
    let report = report_ops(source);
    let rows = rows(&report, "dead_code");
    let start = source.find("typeName as Q").unwrap() as u32;
    assert_eq!(rows, vec![(start, 13)], "{:?}", code_names(&report));
}
