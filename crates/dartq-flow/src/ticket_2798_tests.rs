//! The #2798 flow-cluster pins (co-failure decomposition wave 4,
//! families A3/B3/B4). Upstream citations per fix:
//!
//! * A3 — the is-check branch-death faces. The verdict folds
//!   (`isExpression_end`, upstream Dart SDK source,
//!   pkg/_fe_analyzer_shared/lib/src/flow_analysis/flow_analysis.dart:
//!   7178-7205) and the promotion split's `ifFalseIsUnreachable`
//!   (:4156-4160) already dead arms; the is-CONTINUATION dead row — a
//!   Never-typed operand's `handleExit` (type_analyzer.dart:817-819:
//!   `analyzeExpression` calls `flow.handleExit` when the dispatched
//!   expression's static type is bottom) kills the state mid-expression,
//!   and the dead-code walk's first unreachable node is the tested type
//!   annotation (`dead_code_verifier.dart`'s `_visitNode` at the
//!   `TypeAnnotation` child, its `beginToken`) — anchors at the type
//!   spelling (`DeadCodeTest/test_isExpression_type`: the oracle's row
//!   runs `int;`, from the tested type's head to the covering body's
//!   end).
//! * B3 — the cascade faces (`cascadeExpression_afterTarget`,
//!   flow_analysis.dart:6561-6603): a null-aware `?..` head promotes the
//!   target non-null through the sections (`_nullAwareAccess_rightBegin`)
//!   and joins the receiver-was-null shortcut at the cascade's end
//!   (`nullAwareAccess_end`); field promotions on the cascade target's
//!   temporary reference persist into later sections (:6574-6593); a
//!   section's own `?.` shortcut joins at the section boundary, so
//!   variable promotions made inside the shorted chain do not survive
//!   into the next section.
//! * B4 — the statement-form operand faces the semantic unchecked rows
//!   read (`bool_expression_verifier.dart:45-73` for conditions; the
//!   for-in iterable and spread reads carry their flow type through the
//!   ordinary read rows).

use crate::ev::Ev;
use crate::facts::FlowFact;
use crate::forward::SharedFlow;
use crate::record::Flow;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::{ParserFeatures, TokenSpan};
use dartq_scanner::{ScanConfig, TokenTable};

fn scan_parse(source: &str) -> Vec<FlowFact> {
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
    let mut parser = dartq_parser::Parser::new(source, tokens, features, &mut flow);
    parser.parse_unit();
    drop(parser);
    flow.0.borrow_mut().take_facts()
}

/// Dead-code rows rendered as source snippets.
fn dead_of(source: &str) -> Vec<String> {
    scan_parse(source)
        .into_iter()
        .filter_map(|fact| match fact {
            FlowFact::DeadCode { span } => Some(
                source
                    [span.range.offset as usize..(span.range.offset + span.range.length) as usize]
                    .to_string(),
            ),
            _ => None,
        })
        .collect()
}

/// `(snippet, nullable)` of every read-type row, and the
/// `(snippet, nullable)` condition rows.
fn read_tys(source: &str) -> (Vec<(&str, bool)>, Vec<(&str, bool)>) {
    let facts = scan_parse(source);
    let at = |span: dartq_source::SourceRange| {
        &source[span.offset as usize..(span.offset + span.length) as usize]
    };
    let mut reads = Vec::new();
    let mut conds = Vec::new();
    for fact in facts {
        match fact {
            FlowFact::ReadType { span, ty, .. } => reads.push((at(span.range), ty.nullable)),
            FlowFact::ConditionType { span, ty, .. } => conds.push((at(span.range), ty.nullable)),
            _ => {}
        }
    }
    (reads, conds)
}

/// Whether a promoted-property-read row stands at the given snippet.
fn promoted_prop_at(source: &str, snippet: &str) -> bool {
    let offset = source.find(snippet).expect("snippet exists") as u32;
    scan_parse(source).into_iter().any(|fact| {
        matches!(fact, FlowFact::PromotedPropertyRead { span } if span.range.offset == offset)
    })
}

fn tspan(offset: u32) -> TokenSpan {
    TokenSpan {
        id: offset,
        range: dartq_source::SourceRange { offset, length: 1 },
    }
}

// ————————————————————————— A3: is-check branch death ————————————————————

/// A Never-typed operand's exit inside an is-check opens the dead row at
/// the TESTED TYPE (the official walk's first unreachable node), running
/// to the statement's end — `neverFn() is int;` reports `int;`.
#[test]
fn a3_never_operand_is_check_anchors_dead_row_at_tested_type() {
    // `void f() { neverFn() is int; }` — the operand call `neverFn()`
    // spans 11-19, the exit lands at its end (offset 20), the tested
    // type `int` spans 24-26, the `;` ends at 28. Hand-driven: only the
    // engine resolves a call's type to Never, so the parser pipeline
    // alone cannot build this buffer.
    let events: Vec<Ev> = [
        vec![Ev::ExecBegin { is_closure: false }, Ev::BlockBodyBegin],
        vec![
            // `neverFn()` — its type resolves to Never, the engine
            // records the exit at the call's end (offset 19).
            Ev::OtherExpr { span: tspan(11) },
            Ev::Exit { end: tspan(19) },
            // `is int` — the tested type's head anchors the dead row.
            Ev::IsBegin,
            Ev::Type {
                base: crate::ty::Base::Name(0),
                nullable: false,
                args: crate::ty::EMPTY_ARGS,
            },
            Ev::IsOperator {
                is_not: false,
                type_head: tspan(24),
            },
            Ev::IsEnd,
            Ev::ExprStmt { span: tspan(11) },
            Ev::StmtEnd { end: 28 },
        ],
        vec![
            Ev::BlockBodyEnd,
            Ev::ExecEnd {
                is_expr: false,
                end: tspan(30),
            },
        ],
    ]
    .concat();
    let mut interp = crate::interp::Interp::new(&crate::ty::NameTypeOps, &[], &[]);
    interp.run(&events);
    let dead: Vec<&FlowFact> = interp
        .facts
        .iter()
        .filter(|fact| matches!(fact, FlowFact::DeadCode { .. }))
        .collect();
    assert_eq!(dead.len(), 1, "{:?}", interp.facts);
    match dead[0] {
        FlowFact::DeadCode { span } => {
            // Anchored at the tested type's head, extended to the `;`.
            assert_eq!(span.range.offset, 24, "anchor is the tested type");
            assert_eq!(span.range.length, 4, "the row runs to the `;`");
        }
        other => panic!("expected a DeadCode row, got {other:?}"),
    }
}

/// The known-verdict branch-death faces: a subtype-always-true test deads
/// the failing arm (`x is int` on `int x` — `tryPromoteForTypeCheck`'s
/// bottom `factor` marks the unmatched arm unreachable,
/// flow_analysis.dart:4156-4160), `is!` swaps the dead arm; a
/// nullability-disjoint pair is the constant-false fold
/// (`_isTypeCheckGuaranteedToFailWithSoundNullSafety`, :7184-7189).
#[test]
fn a3_is_verdict_marks_the_failing_branch_dead() {
    // `int x; x is int` always succeeds: the then arm lives, the else
    // arm dies.
    assert_eq!(
        dead_of("void f() { int x = 1; if (x is int) 1; else 2; }"),
        ["2;"]
    );
    // `is!` inverts: the then arm dies.
    assert_eq!(
        dead_of("void f() { int x = 1; if (x is! int) 1; else 2; }"),
        ["1;"]
    );
    // `int is Null` is disjoint: the then arm dies, `is!` the else.
    assert_eq!(
        dead_of("void f() { int x = 1; if (x is Null) 1; else 2; }"),
        ["1;"]
    );
    assert_eq!(
        dead_of("void f() { int x = 1; if (x is! Null) 1; else 2; }"),
        ["2;"]
    );
}

// ————————————————————————— B3: cascade faces ————————————————————————————

/// A null-aware `?..` head promotes the target through the sections and
/// joins the receiver-was-null shortcut at the cascade's end
/// (`cascadeExpression_afterTarget` → `_nullAwareAccess_rightBegin` /
/// `nullAwareAccess_end`, flow_analysis.dart:6561-6603, :7400-7404): the
/// read AFTER the cascade is nullable again.
#[test]
fn b3_null_aware_cascade_target_promotion_ends_with_the_cascade() {
    let source = "void f(C? c) { c?..g(); c.h; }";
    let (reads, _) = read_tys(source);
    // The post-cascade read of `c` (the last row) is nullable.
    let post = reads.last().expect("the post-cascade read carries a row");
    assert_eq!(post.0, "c", "{reads:?}");
    assert!(
        post.1,
        "the ?.. shortcut join demotes the target: {reads:?}"
    );
}

/// Field promotions on the cascade target persist into later sections
/// (`_makeTemporaryReference`'s shared value version — :6574-6593: a
/// `!` in one section keeps the field promoted for the next). The
/// `FieldPromotionTest/test_cascaded_propertyAccess_nullAware` shape:
/// `c?.._field!.toString().._field.toString();` — the second section's
/// `_field` read carries the promoted-property row.
#[test]
fn b3_cascade_section_field_promotion_persists_into_later_sections() {
    let source = "void f(C? c) { c?.._field!.toString().._field.toString(); }";
    assert!(
        promoted_prop_at(source, "_field.toString"),
        "the later section's field read must carry the section-one promotion"
    );
}

/// A section's own `?.` shortcut joins at the section boundary
/// (`nullAwareAccess_end` at the section's close): a variable promoted
/// INSIDE the shorted chain does not survive into the next section —
/// `c..d?.f(i!)..d?.g(i!)` reads `i` nullable again in the second
/// section (the oracle's `unnecessary_non_null_assertion` silence).
#[test]
fn b3_section_null_aware_join_resets_inner_variable_promotion() {
    let source = "void f(C c, int? i) { c..d?.f(i!)..d?.g(i!); }";
    let (reads, _) = read_tys(source);
    let i_reads: Vec<&(&str, bool)> = reads.iter().filter(|(s, _)| *s == "i").collect();
    assert_eq!(i_reads.len(), 2, "{reads:?}");
    // Both `i` reads carry the declared nullable face: the first is read
    // before its own `!` applies, the second after the section join.
    assert!(
        i_reads.iter().all(|(_, nullable)| *nullable),
        "the section join must reset the inner promotion: {reads:?}"
    );
}

// ————————————————————————— B4: statement-form operands ———————————————————

/// The for-in iterable's flow type rides the ordinary read row (the face
/// the semantic as-iterator stage reads): unguarded nullable stays
/// nullable, a null-guard promotion strips it — the official
/// `_checkForEachParts` reads the iterable's flow-aware static type.
#[test]
fn b4_for_in_iterable_read_carries_its_flow_type() {
    let source = "void f() { List? x; for (var y in x) {} }";
    let (reads, _) = read_tys(source);
    let x_row = reads
        .iter()
        .find(|(s, _)| *s == "x")
        .expect("iterable read row");
    assert!(x_row.1, "the nullable iterable stays nullable: {reads:?}");

    let guarded = "void f() { List? x; if (x != null) { for (var y in x) {} } }";
    let (reads, _) = read_tys(guarded);
    // The first `x` row is the condition's own read (nullable there, the
    // promotion applies after it); the for-in iterable read is the last.
    let x_row = reads
        .iter()
        .rev()
        .find(|(s, _)| *s == "x")
        .expect("iterable read row");
    assert!(!x_row.1, "the guarded iterable reads non-null: {reads:?}");
}

/// The spread operand's flow type rides its read row (the in-spread
/// stage's face): `[...list]` over a nullable list reads nullable.
#[test]
fn b4_spread_operand_read_carries_its_flow_type() {
    let source = "void f() { List? list; [...list]; }";
    let (reads, _) = read_tys(source);
    let row = reads
        .iter()
        .find(|(s, _)| *s == "list")
        .expect("spread read row");
    assert!(
        row.1,
        "the nullable spread operand stays nullable: {reads:?}"
    );
}

/// The condition-operand row (official `checkForNonBoolCondition`'s flow
/// half, bool_expression_verifier.dart:45-73): `if (b)` over `bool? b`
/// carries the nullable condition type at the operand read.
#[test]
fn b4_condition_operand_row_for_nullable_bool() {
    let source = "void f(bool? b) { if (b) { 1; } }";
    let (_, conds) = read_tys(source);
    assert_eq!(
        conds,
        [("b", true)],
        "the nullable bool condition carries its flow type"
    );
}
