//! The #2450 pins: the dead-code verifier's invalid-face family (census
//! F2) — the two oracle mechanisms dartq stayed silent on, plus the
//! counter-finding deletion.
//!
//! - Switch absorption over an invalid-scrutinee face: upstream
//!   `promoteForPattern` (upstream Dart SDK source,
//!   pkg/_fe_analyzer_shared/lib/src/flow_analysis/flow_analysis.dart:
//!   7708-7768) marks the matched type covered when
//!   `isSubtypeOf(matched, required)` holds, and `InvalidType` is a
//!   vacuous subtype of everything (subtype.dart:274-276) — the first
//!   guard-free clause whose pattern names a resolvable type absorbs the
//!   match and every following clause reports `dead_code`. The face
//!   fires for a bare scrutinee whose latched type is the unknown face
//!   (`var x = pop();` under an unresolvable receiver — the corpus's
//!   `ast_builder.dart` rows) and for switch expressions (the
//!   `generic_inferrer.dart` rows); a written `dynamic` scrutinee keeps
//!   every clause live (probed: `var x; switch (x.y)`).
//! - The invalid-operand is-check promotion: an operand whose written
//!   annotation resolves nowhere is that `InvalidType`, so `x is int`
//!   folds constant-true (the else deads) and `x is! int` constant-false
//!   (the then deads) — while an UNDEFINED tested type never folds
//!   (#2450 counter-finding): the oracle bails with
//!   `type_test_with_undefined_name` on the spelling and both branches
//!   stay live.
//!
//! Oracle provenance: the census probes `/tmp/probe2446/p9_switchdead`
//! and `p12_isdead` against `dart analyze`, plus the corpus rows
//! (`ast_builder.dart` 1564/1565/1910/6143-6146,
//! `generic_inferrer.dart` 631-638).

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
/// `TypeOps` face mounted — the engine path the is-check folds and the
/// dead-case synthesis re-decide through.
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
    let ops = ResolvedTypeOps::new(&types, &strings, file);
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

// —— (a) switch absorption over the invalid scrutinee ————————————————

/// The p9 face: `var x = pop();` chains the unresolved call's
/// `InvalidType` into the scrutinee, so `case AstNodeImpl a:` absorbs and
/// the following clause reports dead at its `case` keyword and at its
/// first body statement (the corpus's `ast_builder.dart` 1910 rows).
#[test]
fn unknown_scrutinee_switch_deads_the_clauses_after_the_absorber() {
    let source = "\
class AstNodeImpl {
  AstNodeImpl();
}

class Data {
  Data();
}

void f() {
  var x = pop();
  switch (x) {
    case AstNodeImpl a:
      x = 1;
    case Data d:
      x = 2;
  }
}
";
    let report = report_ops(source);
    let dead = rows(&report, "dead_code");
    let second_case = source.find("case Data").unwrap() as u32;
    assert!(
        dead.iter().any(|&(offset, len)| offset == second_case && len == 4),
        "the clause after the absorber reports at its `case` keyword: {:?}",
        report.diagnostics
    );
    let body = source.find("x = 2;").unwrap() as u32;
    assert!(
        dead.iter().any(|&(offset, _)| offset == body),
        "the absorbed clause's body reports dead: {:?}",
        report.diagnostics
    );
}

/// An equality head never covers (`constantPattern_end`'s
/// `_handleEqualityCheckPattern` face — the corpus's
/// `switch (preliminaryName) { case null: … }` rows): the walk skips it
/// and the absorption still lands at the first typed clause, so the rows
/// start after it, not at it.
#[test]
fn constant_head_skips_and_the_absorption_still_lands() {
    let source = "\
class Token {
  Token();
}

class Qualified {
  Qualified();
}

void f() {
  var x = pop();
  switch (x) {
    case null:
      break;
    case Token():
      x = 1;
    case Qualified():
      x = 2;
  }
}
";
    let report = report_ops(source);
    let dead = rows(&report, "dead_code");
    let token_case = source.find("case Token()").unwrap() as u32;
    assert!(
        !dead.iter().any(|&(offset, _)| offset == token_case),
        "the absorbing clause itself stays live: {:?}",
        report.diagnostics
    );
    let qualified_case = source.find("case Qualified()").unwrap() as u32;
    assert!(
        dead.iter().any(|&(offset, len)| offset == qualified_case && len == 4),
        "the clause after the absorber reports at its `case` keyword: {:?}",
        report.diagnostics
    );
    let body = source.find("x = 2;").unwrap() as u32;
    assert!(
        dead.iter().any(|&(offset, _)| offset == body),
        "the absorbed clause's body reports dead: {:?}",
        report.diagnostics
    );
}

/// The dynamic face stays silent (probed: `var x; switch (x.y)` — the
/// oracle keeps every clause live over a `dynamic` scrutinee; a written
/// `dynamic` latches `Ty::Dynamic`, never the unknown face).
#[test]
fn dynamic_scrutinee_keeps_every_clause_live() {
    let source = "\
class A {
  A();
}

class B {
  B();
}

void f(dynamic x) {
  switch (x) {
    case A a:
      x = 1;
    case B b:
      x = 2;
  }
}
";
    let report = report_ops(source);
    assert!(
        rows(&report, "dead_code").is_empty(),
        "a dynamic scrutinee never absorbs: {:?}",
        report.diagnostics
    );
}

/// The switch-expression face (the corpus's `generic_inferrer.dart`
/// 631-638 rows): the arms after the absorbing one report the whole-case
/// row — pattern through the arm expression, trailing comma excluded.
#[test]
fn unknown_scrutinee_switch_expression_deads_the_arms_after_the_absorber() {
    let source = "\
class Res {
  Res();
}

class Inv {
  Inv();
}

void f() {
  var x = pop();
  var e = switch (x) {
    Res r => 1,
    Inv i => 2,
  };
  e;
}
";
    let report = report_ops(source);
    let dead = rows(&report, "dead_code");
    let arm = source.find("Inv i => 2").unwrap() as u32;
    let expected_len = "Inv i => 2".len() as u32;
    assert!(
        dead.iter()
            .any(|&(offset, len)| offset == arm && len == expected_len),
        "the arm after the absorber reports the whole-case row: {:?}",
        report.diagnostics
    );
    let absorbing = source.find("Res r => 1").unwrap() as u32;
    assert!(
        !dead.iter().any(|&(offset, _)| offset == absorbing),
        "the absorbing arm itself stays live: {:?}",
        report.diagnostics
    );
}

/// The switch-expression face over a written invalid receiver (the
/// generic_inferrer shape): the `<unresolvable>.member` receiver's
/// annotation chains the invalid face into the scrutinee.
#[test]
fn invalid_receiver_switch_expression_deads_the_arms_after_the_absorber() {
    let source = "\
class Res {
  Res();
}

class Inv {
  Inv();
}

void f(MissingAnn errorEntity) {
  var e = switch (errorEntity.resolution) {
    Res r => 1,
    Inv i => 2,
  };
  e;
}
";
    let report = report_ops(source);
    let dead = rows(&report, "dead_code");
    let arm = source.find("Inv i => 2").unwrap() as u32;
    assert!(
        dead.iter().any(|&(offset, _)| offset == arm),
        "the arm after the absorber reports dead: {:?}",
        report.diagnostics
    );
}

// —— (b) the invalid-operand is-check promotion ———————————————————————

/// An operand whose written annotation resolves nowhere is the oracle's
/// `InvalidType` — a vacuous subtype of everything — so `x is int` folds
/// constant-true and the else branch reports dead (the corpus's
/// `ast_builder.dart` 1564/1565 conditional-else rows).
#[test]
fn invalid_operand_is_check_deads_the_else() {
    let source = "void f(MissingAnn x) {\n\
                      \x20 if (x is int) {\n\
                      \x20   x = 1;\n\
                      \x20 } else {\n\
                      \x20   x = 2;\n\
                      \x20 }\n\
                      }";
    let report = report_ops(source);
    let dead = rows(&report, "dead_code");
    let else_start = source.find("else").unwrap() as u32;
    assert!(
        dead.iter().any(|&(offset, _)| offset >= else_start),
        "the else arm of an invalid-operand `is int` is dead: {:?}",
        report.diagnostics
    );
}

/// The `is!` polarity swaps the dead branch: the test folds
/// constant-false, so the then branch reports dead.
#[test]
fn invalid_operand_is_not_check_deads_the_then() {
    let source = "void f(MissingAnn x) {\n\
                      \x20 if (x is! int) {\n\
                      \x20   x = 2;\n\
                      \x20 } else {\n\
                      \x20   x = 1;\n\
                      \x20 }\n\
                      }";
    let report = report_ops(source);
    let dead = rows(&report, "dead_code");
    let test_start = source.find("is!").unwrap() as u32;
    let else_start = source.find("else").unwrap() as u32;
    assert!(
        dead.iter()
            .any(|&(offset, _)| offset > test_start && offset < else_start),
        "the then arm of an invalid-operand `is! int` is dead: {:?}",
        report.diagnostics
    );
}

// —— (c) the deleted always-true fold —————————————————————————————————

// 2026-10-22 re-probe (Dart 3.13.1, `dart analyze --format=machine`): the
// both-arms posture this section originally pinned was wrong — the live
// oracle reports the dead-arm row for every undefined-tested shape
// (int/dynamic/invalid operand, both polarities), anchored at the dead
// arm's block. The official reference path never folds to a literal, but
// `tryPromoteForTypeCheck`'s `factor(previous, InvalidType)` is `Never`
// (`subtype.dart:274` makes any relation with an invalid type a
// subtype), so `ifFalseIsUnreachable` fires and the test-false arm is
// unreachable (flow_analysis.dart:4156-4160, :4188-4190).

/// An UNDEFINED tested type keeps the spelling's own code AND deads the
/// test-false arm, even over an invalid operand: `x is Missing` deads
/// the else (the pre-#2450 constant fold over-reported through a
/// boolean-literal face — the dead arm is real, the literal was not).
#[test]
fn invalid_operand_undefined_tested_deads_the_else() {
    let source = "void f(MissingAnn x) {\n\
                      \x20 if (x is Missing) {\n\
                      \x20   x = 1;\n\
                      \x20 } else {\n\
                      \x20   x = 2;\n\
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

/// The deleted fold, `int` operand: `x is Missing` deads the else arm
/// and reports the undefined-name code on the spelling (the flipped
/// dead-family posture, re-pinned against the live oracle).
#[test]
fn int_operand_undefined_tested_deads_the_else() {
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

/// The deleted fold, `is!` polarity: the test answers `true`, so the
/// `is!` then arm is the dead one.
#[test]
fn is_not_undefined_tested_deads_the_then() {
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

/// The undefined tested type never promotes either: the then branch of
/// `x is Missing` reads `x` under its declared face — the promotion gate
/// refused the step (`try_promote_to_typed` bails on the undefined
/// head), so no promotion fact rides the branch.
#[test]
fn undefined_tested_type_promotes_nothing() {
    let source = "void f(int x) {\n\
                      \x20 if (x is Missing) {\n\
                      \x20   x.foot;\n\
                      \x20 }\n\
                      }";
    let report = report_ops(source);
    assert!(
        rows(&report, "dead_code").is_empty(),
        "the then branch stays live: {:?}",
        report.diagnostics
    );
    assert!(
        code_names(&report)
            .iter()
            .any(|name| *name == "undefined_getter"),
        "the member read resolves against the declared `int` face: {:?}",
        report.diagnostics
    );
}
