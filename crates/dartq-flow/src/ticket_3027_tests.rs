//! #3027 — `UnassignedRead` metadata and join pins for the shared-case /
//! late-local / final faces, mirroring the official reader (upstream
//! Dart SDK source, `pkg/analyzer/lib/src/generated/resolver.dart:655-702`,
//! `checkReadOfNotAssignedLocalVariable`): `late` locals report only on
//! the `isDefinitelyUnassigned` bit; non-late `!isAssigned` reads report
//! `read_potentially_unassigned_final` when the element is final, else
//! `not_assigned_potentially_non_nullable_local_variable` under the
//! `isPotentiallyNonNullable` gate. The flow row carries every gate input
//! (`is_late`/`is_final`/`definitely_unassigned`/
//! `potentially_non_nullable`); the consumer picks its own gate.
//!
//! Two metadata defects this pins shut:
//!
//! * The stacked-head binder alias (official `switchStatement_
//!   endAlternative`'s `copyPromotionData`, `flow_analysis.dart:6660-6686`):
//!   a later head of the same shared-body group re-declaring a bound name
//!   aliases to the FIRST head's variable — the shared body reads ONE
//!   model, assigned on every matched head. Without it the body resolved
//!   the LAST head's binder, fresh-unassigned on the earlier heads'
//!   matched paths, and a spurious potentially-unassigned row fired over
//!   a body the oracle reads as assigned.
//! * The `var`-binder finality bit: the parser parks both `var` and
//!   `final` in the declared-variable pattern's keyword slot, so presence
//!   alone marked every `var` binder final (oracle-probed 3.13.7:
//!   `if (v case var a) { a = 1; }` assigns legally; only `final` reports
//!   `assignment_to_final_local`).
//!
//! Oracle: `dart analyze` 3.13.7 over the fixture faces below (the
//! shared-case pattern-variable reads the oracle masks at the resolver
//! through `invalid_pattern_variable_in_shared_case_scope` — the flow
//! model's potentially-unassigned posture beneath the mask is what these
//! rows pin).

use crate::facts::FlowFact;
use crate::forward::SharedFlow;
use crate::record::Flow;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_scanner::{ScanConfig, TokenTable};

/// The unassigned-read rows of `source`: each read's snippet with its
/// four gate bits (`is_late`, `is_final`, `definitely_unassigned`,
/// `potentially_non_nullable`).
fn unassigned_rows(source: &str) -> Vec<(String, bool, bool, bool, bool)> {
    let result = dartq_scanner::scan(
        source,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        dartq_source::FileTable::new().intern(std::path::Path::new("test.dart")),
    );
    assert!(result.diagnostics.is_empty(), "{source:?}");
    let tokens: &'static TokenTable = Box::leak(Box::new(result.tokens));
    let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(
        LanguageVersion::new(3, 15),
    ));
    let mut flow = SharedFlow(std::rc::Rc::new(std::cell::RefCell::new(Flow::new(
        tokens, source,
    ))));
    flow.0
        .borrow_mut()
        .set_sound_flow(features.sound_flow_analysis);
    let mut parser = dartq_parser::Parser::new(source, tokens, features, &mut flow);
    parser.parse_unit();
    drop(parser);
    flow.0
        .borrow_mut()
        .take_facts()
        .into_iter()
        .filter_map(|fact| match fact {
            FlowFact::UnassignedRead {
                span,
                is_late,
                is_final,
                definitely_unassigned,
                potentially_non_nullable,
                ..
            } => Some((
                source[span.range.offset as usize..span.range.end() as usize].to_string(),
                is_late,
                is_final,
                definitely_unassigned,
                potentially_non_nullable,
            )),
            _ => None,
        })
        .collect()
}

/// One expected row, spelled out for readability.
fn row(
    snippet: &str,
    late: bool,
    final_: bool,
    unassigned: bool,
    pnn: bool,
) -> (String, bool, bool, bool, bool) {
    (snippet.to_string(), late, final_, unassigned, pnn)
}

// —— the plain join faces ————————————————————————————————————————————

#[test]
fn join_potentially_unassigned_non_final() {
    // `int x; if (b) x = 1; print(x)` — the join face: assigned on one
    // path only, so neither definitely-unassigned nor assigned (oracle:
    // not_assigned_potentially_non_nullable_local_variable at the read).
    let src = "void f(bool b) {\n  int x;\n  if (b) {\n    x = 1;\n  }\n  print(x);\n}";
    assert_eq!(
        unassigned_rows(src),
        vec![row("x", false, false, false, true)]
    );
}

#[test]
fn final_potentially_unassigned_join() {
    // `final int y; if (b) y = 1; print(y)` — the same join over a final:
    // is_final routes the consumer to read_potentially_unassigned_final
    // (no nullability gate needed).
    let src = "void f(bool b) {\n  final int y;\n  if (b) {\n    y = 1;\n  }\n  print(y);\n}";
    assert_eq!(
        unassigned_rows(src),
        vec![row("y", false, true, false, true)]
    );
}

#[test]
fn final_definitely_unassigned() {
    // `final int y; print(y)` — never written: both bits stand.
    let src = "void f() {\n  final int y;\n  print(y);\n}";
    assert_eq!(
        unassigned_rows(src),
        vec![row("y", false, true, true, true)]
    );
}

#[test]
fn final_read_after_write_silent() {
    // `final int y; y = 1; print(y)` — definitely assigned on every path
    // into the read: no row (oracle silent).
    let src = "void f() {\n  final int y;\n  y = 1;\n  print(y);\n}";
    assert_eq!(unassigned_rows(src), Vec::new());
}

// —— the late-local faces ————————————————————————————————————————————

#[test]
fn late_definitely_unassigned() {
    // `late int w; print(w)` — the late gate's own bit: the consumer
    // reports definitely_unassigned_late_local_variable.
    let src = "void f() {\n  late int w;\n  print(w);\n}";
    assert_eq!(
        unassigned_rows(src),
        vec![row("w", true, false, true, true)]
    );
}

#[test]
fn late_final_definitely_unassigned() {
    // `late final` reports through the late gate first (the official
    // checks isLate before isFinal), so both bits ride the row.
    let src = "void f() {\n  late final int w;\n  print(w);\n}";
    assert_eq!(unassigned_rows(src), vec![row("w", true, true, true, true)]);
}

#[test]
fn late_potentially_assigned_carries_false_bit() {
    // `late int w; if (b) w = 1; print(w)` — assigned on one path: the
    // late consumer stays SILENT (its gate reads the unassigned bit
    // only), so the row must carry definitely_unassigned = false.
    let src = "void f(bool b) {\n  late int w;\n  if (b) {\n    w = 1;\n  }\n  print(w);\n}";
    assert_eq!(
        unassigned_rows(src),
        vec![row("w", true, false, false, true)]
    );
}

// —— the shared-body plain local ——————————————————————————————————————

#[test]
fn shared_case_plain_local_definitely_unassigned() {
    // `case 1: case 2: int z; print(z)` — a plain local declared inside
    // the shared body starts unassigned on the merged body state (oracle:
    // not_assigned_potentially_non_nullable_local_variable).
    let src = "void f(int v) {\n  switch (v) {\n    case 1:\n    case 2:\n      int z;\n      print(z);\n  }\n}";
    assert_eq!(
        unassigned_rows(src),
        vec![row("z", false, false, true, true)]
    );
}

// —— the stacked-head binder alias (copyPromotionData) ————————————————

#[test]
fn shared_case_consistent_heads_read_assigned() {
    // `case (int a, int b): case (int a, int b): print(a + b)` — both
    // heads bind the same names consistently, so the shared body's reads
    // address the FIRST head's variables, declared initialized on every
    // matched head: no row (oracle reports nothing but
    // unreachable_switch_case here). Without the alias the body resolved
    // the LAST head's binders and reported them potentially unassigned.
    let src = "void f(Object v) {\n  switch (v) {\n    case (int a, int b):\n    case (int a, int b):\n      print(a + b);\n  }\n}";
    assert_eq!(unassigned_rows(src), Vec::new());
}

#[test]
fn shared_case_consistent_irrefutable_heads_read_assigned() {
    // `case var a: case var a: print(a)` — the `var`/`var` consistent
    // pair: the second head's `a` aliases the first's; the dead second
    // head joins nothing, and the body reads the first head's assigned
    // binder (oracle: only dead_code/unreachable_switch_case rows).
    let src = "void f(Object v) {\n  switch (v) {\n    case var a:\n    case var a:\n      print(a);\n  }\n}";
    assert_eq!(unassigned_rows(src), Vec::new());
}

#[test]
fn guarded_consistent_heads_read_assigned() {
    // `case (int a, int b) when a > b: case (int a, int b): print(a + b)`
    // — a guard on the first head keeps the second reachable, so the
    // group close joins BOTH matched arms; the alias keeps the binder
    // assigned on each (the guard's own reads run on the matched path and
    // report nothing).
    let src = "void f(Object v) {\n  switch (v) {\n    case (int a, int b) when a > b:\n    case (int a, int b):\n      print(a + b);\n  }\n}";
    assert_eq!(unassigned_rows(src), Vec::new());
}

// —— the `var`-binder finality bit ————————————————————————————————————

#[test]
fn labeled_shared_case_var_binder_is_not_final() {
    // `case var a: L: case var b: print(a)` — the label forces the
    // official labeled-member close (a conservative join off the switch
    // entry, where the first head's binder has no model), so the read IS
    // a fact-level potentially/definitely-unassigned row — and a `var`
    // binder's is_final must be FALSE (pre-fix the keyword's presence
    // marked it final). The oracle masks the read at the resolver
    // (`invalid_pattern_variable_in_shared_case_scope`, label face); the
    // flow model beneath the mask is what this row pins.
    let src = "void f(Object v) {\n  switch (v) {\n    case var a:\n    L: case var b:\n      print(a);\n  }\n}";
    assert_eq!(
        unassigned_rows(src),
        vec![row("a", false, false, true, false)]
    );
}

#[test]
fn null_check_binder_read_assigned() {
    // `case var x?: print(x)` — the null-check binder is declared on the
    // matched path the read runs under: no row.
    let src = "void f(Object? o) {\n  switch (o) {\n    case var x?:\n      print(x);\n  }\n}";
    assert_eq!(unassigned_rows(src), Vec::new());
}
