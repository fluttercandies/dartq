//! End-to-end pins for the null-check/null-assert pattern leaf space
//! (#2354): `p?` covers the subpattern space ∩ nonNull — never `null`
//! itself (official `createNullCheckSpace`, upstream Dart SDK source,
//! pkg/_fe_analyzer_shared/lib/src/exhaustiveness/shared.dart:780-786);
//! `p!` unions that space with `null` (official `createNullAssertSpace`,
//! shared.dart:790-797). The pre-fix construction had the two swapped:
//! `p?` produced the nullable leaf (wrongly killing a following
//! `case null:`) and `p!` dropped the null side (wrongly keeping later
//! cases live).

use std::path::Path;

use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::{Combinators, ImportEdge, LibraryGraphBuilder, Namespaces};
use dartq_scanner::{ScanConfig, scan};
use dartq_semantic::MicroKind;
use dartq_semantic::diagnose::{StrictModes, UnitFold, fold_unit};
use dartq_semantic::library::ElementIndex;
use dartq_source::{FileTable, StringTable};

const CORE_FACE: &[&str] = &["Object", "bool", "double", "int", "String", "Null"];

/// Runs `source` through the live pipeline (the corpus posture: language
/// 3.13, a standalone library over the synthetic core face) and returns
/// the whole fold.
fn unit_fold<'s>(source: &'s str, strings: &StringTable) -> UnitFold {
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 13));
    let scanned = scan(
        source,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: set.is_enabled(dartq_language::Feature::Augmentations),
        },
        file,
    );
    assert!(
        scanned.diagnostics.is_empty(),
        "scanner diagnostics: {scanned:?}"
    );
    let features = dartq_parser::ParserFeatures::from_feature_set(set);
    let elements = dartq_semantic::collector::collect(source, &scanned.tokens, features, strings);
    let mut builder = LibraryGraphBuilder::new(strings);
    let lib = builder.intern("package:t/main.dart");
    let core_file = files.intern(Path::new("<dart:core>"));
    let core = builder.intern("dart:core");
    for name in CORE_FACE {
        builder.declare(core, strings.intern(name), core_file);
    }
    builder.import(
        lib,
        ImportEdge {
            target: Some(core),
            prefix: None,
            deferred: false,
            combinators: Combinators::unfiltered(),
        },
    );
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
    fold_unit(
        source,
        &scanned.tokens,
        features,
        strings,
        file,
        lib,
        &graph,
        &namespaces,
        &index,
        StrictModes::default(),
        &dartq_resolution::LibraryClassModifiers::default(),
        set.is_enabled(dartq_language::Feature::InferenceUpdate2),
    )
}

/// The switch-reachability rows: `(code, offset)` pairs for the two codes
/// the leaf space drives (`unreachable_switch_case` from the
/// exhaustiveness face, `dead_code` from the dead-chain face), offset in
/// UTF-16 units (the ASCII pin sources make that byte-equal).
fn switch_rows(fold: &UnitFold) -> Vec<(&'static str, u32)> {
    fold.report
        .diagnostics
        .iter()
        .filter_map(|d| match d.code.name.as_ref() {
            "unreachable_switch_case" => Some(("unreachable_switch_case", d.range.offset)),
            "dead_code" => Some(("dead_code", d.range.offset)),
            _ => None,
        })
        .collect()
}

/// Direction 1 (the riverpod `notifier_provider.dart` shape): over a
/// nullable named scrutinee, `case final v?:` does NOT cover `null`, so
/// the following `case null:` stays live and the switch exhausts without
/// any dead rows.
#[test]
fn null_check_leaf_keeps_case_null_live() {
    let source = "\
class Box {}
void f(Box? x) {
  switch (x) {
    case final v?:
      v;
    case null:
      break;
  }
}
";
    let mut strings = StringTable::default();
    let fold = unit_fold(source, &strings);
    assert!(
        switch_rows(&fold).is_empty(),
        "expected a live `case null:`, got {:?}",
        switch_rows(&fold)
    );
}

/// Direction 2 (the `bang/b.dart` shape): `case var y!:` covers
/// `int ∪ null` — the whole scrutinee — so all three later cases are
/// unreachable, each flagged on its `case` keyword (the dead-chain face
/// flags the header: `dead_code` over scalar scrutinees,
/// `unreachable_switch_case` over named ones — same verdict either way).
#[test]
fn null_assert_leaf_covers_null_and_kills_later_cases() {
    let source = "\
void f(int? x) {
  switch (x) {
    case var y!:
      break;
    case null:
      break;
    case 1:
      break;
    case 2:
      break;
  }
}
";
    let case_bang = source.find("case var y!").unwrap() as u32;
    let case_null = source.find("case null").unwrap() as u32;
    let case_one = source.find("case 1").unwrap() as u32;
    let case_two = source.find("case 2").unwrap() as u32;
    let mut strings = StringTable::default();
    let fold = unit_fold(source, &strings);
    let rows = switch_rows(&fold);
    let offsets: Vec<u32> = rows.iter().map(|(_, off)| *off).collect();
    for off in [case_null, case_one, case_two] {
        assert!(
            offsets.contains(&off),
            "expected a dead row at offset {off}, got {rows:?}"
        );
    }
    assert!(
        !offsets.contains(&case_bang),
        "the `p!` case itself must stay live, got {rows:?}"
    );
}

/// Direction 3 (the `pat.dart` shape): `var y?` over an `Object?`
/// scrutinee yields the non-null `Object` leaf — the arm alone never
/// exhausts the switch, so the following `case null:` produces no dead
/// rows (the pre-fix nullable leaf wrongly killed it).
#[test]
fn null_check_alone_never_exhausts_object_question_scrutinee() {
    let source = "\
void f(Object? x) {
  switch (x) {
    case var y?:
      y;
    case null:
      x;
  }
}
";
    let mut strings = StringTable::default();
    let fold = unit_fold(source, &strings);
    assert!(
        switch_rows(&fold).is_empty(),
        "expected no dead rows over the `Object?` scrutinee, got {:?}",
        switch_rows(&fold)
    );
}

/// A guard keeps its case out of the covering set regardless of the leaf
/// space: both later cases stay live behind a guarded `p?`.
#[test]
fn guarded_null_check_keeps_following_cases_live() {
    let source = "\
void f(int? x) {
  switch (x) {
    case var y? when y > 0:
      break;
    case 1:
      break;
    case null:
      break;
  }
}
";
    let mut strings = StringTable::default();
    let fold = unit_fold(source, &strings);
    assert!(
        switch_rows(&fold).is_empty(),
        "expected all cases live behind the guard, got {:?}",
        switch_rows(&fold)
    );
}

/// Same guard face for `p!`: the guarded arm's space (which now covers
/// `null`) must not kill the following `case null:`.
#[test]
fn guarded_null_assert_keeps_following_cases_live() {
    let source = "\
void f(int? x) {
  switch (x) {
    case var y! when y > 0:
      break;
    case null:
      break;
  }
}
";
    let mut strings = StringTable::default();
    let fold = unit_fold(source, &strings);
    assert!(
        switch_rows(&fold).is_empty(),
        "expected all cases live behind the guard, got {:?}",
        switch_rows(&fold)
    );
}

/// Boundary: over a NON-nullable scrutinee the `p?` leaf is the context
/// type itself — already total — so a following `case null:` is dead (the
/// arm can never fail); the `p?` case itself stays live.
#[test]
fn null_check_over_non_nullable_scrutinee_exhausts_before_null_case() {
    let source = "\
void f(int x) {
  switch (x) {
    case var y?:
      break;
    case null:
      break;
  }
}
";
    let case_check = source.find("case var y?").unwrap() as u32;
    let case_null = source.find("case null").unwrap() as u32;
    let mut strings = StringTable::default();
    let fold = unit_fold(source, &strings);
    let rows = switch_rows(&fold);
    let offsets: Vec<u32> = rows.iter().map(|(_, off)| *off).collect();
    assert!(
        offsets.contains(&case_null),
        "expected `case null:` dead, got {rows:?}"
    );
    assert!(
        !offsets.contains(&case_check),
        "the `p?` case itself must stay live, got {rows:?}"
    );
}

/// Non-variable scrutinee: the same leaf space applies to a member access
/// — `case final v?:` leaves the `case null:` live.
#[test]
fn null_check_leaf_on_member_scrutinee_keeps_case_null_live() {
    let source = "\
class Box {
  Box? next;
}
void f(Box b) {
  switch (b.next) {
    case final v?:
      v;
    case null:
      break;
  }
}
";
    let mut strings = StringTable::default();
    let fold = unit_fold(source, &strings);
    assert!(
        switch_rows(&fold).is_empty(),
        "expected a live `case null:` behind the member scrutinee, got {:?}",
        switch_rows(&fold)
    );
}
