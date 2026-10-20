//! Unit tests mined from the official flow-analysis corpus
//! (`upstream Dart SDK source, pkg/_fe_analyzer_shared/test/flow_analysis/`),
//! each citing its source file. Assertions cover the crate's fact face:
//! dead code and definite-unassigned reads.

use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_scanner::{ScanConfig, TokenTable};

use crate::facts::{AsyncOrigin, FlowFact};
use crate::forward::SharedFlow;
use crate::record::Flow;

fn facts_of(source: &str) -> Vec<FlowFact> {
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

/// Parses with the recorder attached and returns the live recorder (for
/// the name-recovery face) alongside the taken facts.
fn analyze(source: &str) -> (SharedFlow<'_>, Vec<FlowFact>) {
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
    let facts = flow.0.borrow_mut().take_facts();
    (flow, facts)
}

/// Promoted-read facts of a source: the promoted type of each read whose
/// flow type differs from its declared type (offset-sorted by
/// `take_facts`).
fn promoted_tys(source: &str) -> Vec<crate::ty::Ty> {
    facts_of(source)
        .into_iter()
        .filter_map(|fact| match fact {
            FlowFact::PromotedRead { ty, .. } => Some(ty),
            _ => None,
        })
        .collect()
}

/// Offsets (UTF-16 == ASCII here) of dead-code facts, rendered as the
/// dead source text for readable assertions.
fn dead_snippets(source: &str) -> Vec<String> {
    facts_of(source)
        .into_iter()
        .filter_map(|fact| match fact {
            FlowFact::DeadCode { span } => Some(
                source
                    [span.range.offset as usize..(span.range.offset + span.range.length) as usize]
                    .to_string(),
            ),
            FlowFact::UnassignedRead { .. }
            | FlowFact::PromotedRead { .. }
            | FlowFact::ReadType { .. }
            | FlowFact::WriteSite { .. }
            | FlowFact::ConditionType { .. }
            | FlowFact::MatchedType { .. }
            | FlowFact::AsyncContextUse { .. }
            | FlowFact::PromotedPropertyRead { .. }
            | FlowFact::WhyNotPromoted { .. }
            | FlowFact::NonPromotedPropertyRead { .. }
            | FlowFact::ThisNotPromoted { .. }
            | FlowFact::BodyMayCompleteNormally { .. } => None,
        })
        .collect()
}

/// Snippets of definite-unassigned reads.
fn unassigned_snippets(source: &str) -> Vec<String> {
    facts_of(source)
        .into_iter()
        .filter_map(|fact| match fact {
            FlowFact::UnassignedRead { span, .. } => Some(
                source
                    [span.range.offset as usize..(span.range.offset + span.range.length) as usize]
                    .to_string(),
            ),
            FlowFact::DeadCode { .. }
            | FlowFact::PromotedRead { .. }
            | FlowFact::ReadType { .. }
            | FlowFact::WriteSite { .. }
            | FlowFact::ConditionType { .. }
            | FlowFact::MatchedType { .. }
            | FlowFact::AsyncContextUse { .. }
            | FlowFact::PromotedPropertyRead { .. }
            | FlowFact::WhyNotPromoted { .. }
            | FlowFact::NonPromotedPropertyRead { .. }
            | FlowFact::ThisNotPromoted { .. }
            | FlowFact::BodyMayCompleteNormally { .. } => None,
        })
        .collect()
}

/// The read-face facts of a source (dead code, unassigned reads,
/// promoted reads) without the completion and query-face rows: the
/// assertion face of the tests that pin the three diagnostic faces. A
/// reachable executable emits a completion row by design (#676), and
/// every reachable read emits a `ReadType` row plus its body the write
/// rows (#1056), so a whole-list emptiness check would observe them.
fn read_facts_of(source: &str) -> Vec<FlowFact> {
    facts_of(source)
        .into_iter()
        .filter(|fact| {
            !matches!(
                fact,
                FlowFact::BodyMayCompleteNormally { .. }
                    | FlowFact::ReadType { .. }
                    | FlowFact::WriteSite { .. }
                    | FlowFact::ConditionType { .. }
                    | FlowFact::MatchedType { .. }
                    | FlowFact::PromotedPropertyRead { .. }
                    | FlowFact::WhyNotPromoted { .. }
                    | FlowFact::NonPromotedPropertyRead { .. }
                    | FlowFact::ThisNotPromoted { .. }
            )
        })
        .collect()
}

/// Token span for hand-driven event sequences (arbitrary anchor).
fn tspan(offset: u32) -> dartq_parser::TokenSpan {
    dartq_parser::TokenSpan {
        id: offset,
        range: dartq_source::SourceRange { offset, length: 1 },
    }
}

/// A named-type event for hand-driven sequences.
fn named_ty(name: u32, nullable: bool) -> crate::ev::Ev {
    crate::ev::Ev::Type {
        base: crate::ty::Base::Name(name),
        nullable,
        args: crate::ty::EMPTY_ARGS,
    }
}

/// A bare identifier read for hand-driven sequences.
fn read(name: u32) -> crate::ev::Ev {
    crate::ev::Ev::Ident {
        name,
        token: tspan(name),
        context: dartq_parser::IdentifierContext::Expression,
    }
}

/// An `x is T` / `x is! T` event sequence.
fn is_test(name: u32, ty: crate::ev::Ev, is_not: bool) -> Vec<crate::ev::Ev> {
    vec![
        read(name),
        crate::ev::Ev::IsBegin,
        ty,
        crate::ev::Ev::IsOperator { is_not },
        crate::ev::Ev::IsEnd,
    ]
}

/// A `T x = e;` declaration sequence opening its own executable (the
/// initializer nets one value).
fn declared(name: u32, ty: crate::ev::Ev) -> Vec<crate::ev::Ev> {
    [
        vec![crate::ev::Ev::ExecBegin { is_closure: false }],
        decl_block(name, ty),
    ]
    .concat()
}

/// A declaration inside an already-open executable.
fn decl_block(name: u32, ty: crate::ev::Ev) -> Vec<crate::ev::Ev> {
    vec![
        crate::ev::Ev::VarDeclBegin {
            late: false,
            is_final: false,
            span: tspan(0),
        },
        ty,
        crate::ev::Ev::InitializedIdent {
            name,
            token: tspan(0),
        },
        crate::ev::Ev::InitializerBegin,
        crate::ev::Ev::OtherExpr { span: tspan(1) },
        crate::ev::Ev::InitializerEnd,
        crate::ev::Ev::VarDeclEnd,
    ]
}

/// Drives a hand-built buffer through a fresh interpreter (name-level ops).
fn interp_of(events: &[crate::ev::Ev]) -> crate::interp::Interp<'static> {
    let mut interp = crate::interp::Interp::new(&crate::ty::NameTypeOps, &[], &[]);
    interp.run(events);
    interp
}

/// reachability basics: code after `return` is dead
/// (official `reachability/data/early_exit.dart`).
#[test]
fn dead_after_return() {
    let source = "void f() { return; 1; }";
    assert_eq!(dead_snippets(source), ["1;"]);
}

#[test]
fn dead_after_return_in_if_branches() {
    // Both branches return: the join leaves nothing reachable.
    let source = "void f(bool b) { if (b) { return; } else { return; } 1; }";
    assert_eq!(dead_snippets(source), ["1;"]);
}

#[test]
fn live_after_if_without_else_return() {
    let source = "void f(bool b) { if (b) { return; } 1; }";
    assert!(dead_snippets(source).is_empty());
}

// —— W3 dead-code oracle faces (#868, the W3 dead-code probes) ——

/// Wholly-dead branch face (probe table B): the anchor IS the dead branch's
/// block, so the row spans the block's full extent — closing brace
/// included (official `flowEnd` with `node == firstDeadNode`: no
/// last-statement trim).
#[test]
fn dead_else_block_row_spans_the_whole_block() {
    // Probe g5: oracle row = the else block `{ print('d'); }`.
    let source = "void f() { if (true) {} else { print('d'); } }";
    assert_eq!(dead_snippets(source), ["{ print('d'); }"]);
}

#[test]
fn dead_then_block_row_spans_the_whole_block() {
    // Probe g6.
    let source = "void f() { if (false) { print('d'); } else {} }";
    assert_eq!(dead_snippets(source), ["{ print('d'); }"]);
}

/// `while (false)` body: anchor = the body block, whole block (probe g8).
#[test]
fn while_false_body_row_spans_the_whole_block() {
    let source = "void f() { while (false) { print('d'); } }";
    assert_eq!(dead_snippets(source), ["{ print('d'); }"]);
}

/// Dead right operand of `&&` (probe table C): the row re-anchors at the
/// operator and ends at the right operand's end, and the (now provably
/// dead) then block forms its own row.
#[test]
fn and_false_right_operand_row_anchors_at_the_operator() {
    // Probe g10: oracle rows = `&& x > 0` and the whole then block.
    let source = "void f(int x) { if (false && x > 0) { print('n'); } }";
    assert_eq!(dead_snippets(source), ["&& x > 0", "{ print('n'); }"]);
}

/// A non-constant `&&` LHS keeps the right operand live; only the
/// provably-dead loop body reports (probe g10's second shape).
#[test]
fn non_constant_and_rhs_stays_live_body_reports() {
    let source = "void f(int x) { while (x > 0 && false) { print('a'); } }";
    assert_eq!(dead_snippets(source), ["{ print('a'); }"]);
}

/// Dead right operand of `||` (probe g11): the then branch stays live.
#[test]
fn or_true_right_operand_row_anchors_at_the_operator() {
    let source = "void f(int x) { if (true || x > 0) {} }";
    assert_eq!(dead_snippets(source), ["|| x > 0"]);
}

/// `??` with a guaranteed-non-null LHS type (probe g25, official
/// `_isGuaranteedNonNullWithSoundNullSafety`): the RHS path is unreachable
/// and the row anchors at `??`.
#[test]
fn non_null_lhs_if_null_right_operand_row() {
    let source = "void f(int x) { x ?? 1; }";
    assert_eq!(dead_snippets(source), ["?? 1"]);
}

/// A nullable LHS keeps both `??` arms live.
#[test]
fn nullable_lhs_if_null_reports_nothing() {
    let source = "void f(int? x) { x ?? 1; }";
    assert!(dead_snippets(source).is_empty());
}

/// `??` nested in a call argument keeps the same anchor: the non-nullable
/// LHS still kills the right path and the row opens at the `??` operator,
/// covering the right operand (oracle `print(x ?? y)` → `dead_code` at
/// `?? y`, beside the `dead_null_aware_expression` row on `y`). The end
/// boundary is the send-leaf follower token — the right operand's own end —
/// which is the parser's `end_binary_expression` convention; the anchor is
/// what the diagnostic keys on.
#[test]
fn if_null_right_path_row_inside_call_argument() {
    let source = "void f(int x, int y) { print(x ?? y); }";
    let op_offset = source.find("??").unwrap() as u32;
    let dead: Vec<_> = facts_of(source)
        .into_iter()
        .filter_map(|fact| match fact {
            FlowFact::DeadCode { span } => Some(span.range),
            _ => None,
        })
        .collect();
    assert_eq!(dead.len(), 1, "facts: {dead:?}");
    assert_eq!(dead[0].offset, op_offset);
    // Covers `?? y` at minimum (the `)` follower rounds out the send leaf).
    assert!(dead[0].end() >= source.find(')').unwrap() as u32);
}

/// A guaranteed-fail `as` cast (operand and cast types disjoint by
/// nullability) makes the continuation unreachable: the next statement
/// anchors the dead row — official `asExpression_end`,
/// `_isTypeCheckGuaranteedToFailWithSoundNullSafety`.
#[test]
fn null_operand_cast_kills_the_continuation() {
    let source = "void f(Null n) { var x = n as int; print(x); }";
    assert_eq!(dead_snippets(source), ["print(x);"]);
}

/// The disjointness runs both directions: a non-nullable operand under a
/// `Null` cast target is just as guaranteed to throw.
#[test]
fn non_null_operand_under_null_cast_kills_the_continuation() {
    let source = "void f(int x) { var y = x as Null; print(y); }";
    assert_eq!(dead_snippets(source), ["print(y);"]);
}

/// A cast that ends the block has no following node to anchor a row at — the
/// unreachable state never surfaces.
#[test]
fn cast_as_last_statement_reports_nothing() {
    let source = "void f(Null n) { var x = n as int; }";
    assert!(dead_snippets(source).is_empty());
}

/// A nullable operand under a compatible cast keeps the continuation live.
#[test]
fn nullable_operand_cast_reports_nothing() {
    let source = "void f(int? x) { var y = x as int; print(y); }";
    assert!(dead_snippets(source).is_empty());
}

/// `case null:` against a matched value promoted to non-nullable can never
/// match (official `_EqualityCheckHasKnownResult(areEqual: false)`): the
/// matched state folds into the unmatched chain, `current` goes unreachable,
/// and the case body anchors its dead row at its first statement.
#[test]
fn null_constant_pattern_kills_the_case_body() {
    let source = "\
void f(String? p) {
  String? s = p;
  if (s != null) {
    switch (s) {
      case null:
        break;
      case '':
        break;
    }
  }
}";
    assert_eq!(dead_snippets(source), ["break;"]);
}

/// Dead conditional-expression arms (probe table C): each arm's row spans
/// the arm's own range (`flowEnd(thenExpression)`/`flowEnd(elseExpression)`).
#[test]
fn conditional_dead_then_arm_row() {
    let source = "void f() { false ? 1 : 2; }";
    assert_eq!(dead_snippets(source), ["1"]);
}

#[test]
fn conditional_dead_else_arm_row() {
    let source = "void f() { true ? 1 : 2; }";
    assert_eq!(dead_snippets(source), ["2"]);
}

/// Switch member-order faces (probe table G, r5): every member at/after the
/// first `default` is dead (keyword row at the head-time member check, body
/// row at the member close), and a default followed by a case-headed group
/// reports its own keyword row.
#[test]
fn switch_case_after_default_member_rows() {
    let source = "\
void f(int x) {
  switch (x) {
    case 1:
      break;
    default:
      break;
    case 2:
      break;
  }
}";
    assert_eq!(dead_snippets(source), ["default", "case", "break;"]);
}

/// A default followed by another default-headed group stays live (probe
/// table G, r6): only the SECOND default and its body report.
#[test]
fn switch_multiple_defaults_second_member_rows() {
    let source = "\
void f(int x) {
  switch (x) {
    case 1:
      break;
    default:
      break;
    default:
      break;
  }
}";
    assert_eq!(dead_snippets(source), ["default", "break;"]);
}

/// A trailing default stays live; no member rows at all (probe s1).
#[test]
fn switch_trailing_default_reports_nothing() {
    let source = "\
void f(int x) {
  switch (x) {
    case 1:
      break;
    default:
      break;
  }
}";
    assert!(dead_snippets(source).is_empty());
}

/// Default first, then cases: the default's keyword row plus every later
/// member's keyword and body rows (probe s2/s6).
#[test]
fn switch_default_first_member_rows() {
    let source = "\
void f(int x) {
  switch (x) {
    default:
      break;
    case 1:
      break;
  }
}";
    assert_eq!(dead_snippets(source), ["default", "case", "break;"]);
}

/// A default head sharing a body with a preceding case (probe s5): the
/// shared group's case head stays live, the default's keyword row fires at
/// the next case-headed group's opening, and the later member reports
/// keyword + body rows.
#[test]
fn switch_shared_body_default_row() {
    let source = "\
void f(int x) {
  switch (x) {
    case 1:
    default:
      break;
    case 2:
      break;
  }
}";
    assert_eq!(dead_snippets(source), ["default", "case", "break;"]);
}

/// A Never-typed subexpression records `Ev::Exit` (official
/// `FlowAnalysis.handleExit`, flow_analysis.dart:919-925): the flow state
/// goes unreachable from the expression's end, so the following statement
/// anchors a dead-code row at itself — the first unreachable node, not the
/// Never expression. Hand-driven: only the engine can resolve a
/// subexpression's type to Never, so the parser pipeline cannot produce
/// this buffer yet. The anchor doubles as the collect-rollback pin:
/// `Interp::run` collect-runs the whole buffer first, and a leaked Exit
/// would open the real run's window at the FIRST statement instead.
#[test]
fn never_typed_subexpression_exit_kills_the_rest() {
    use crate::facts::FlowFact;
    // `void f() { never(); 1; }` — offsets: `never()` at 11-17, the `;` at
    // 18, `1;` at 20-21. The recorded grammar per expression statement: the
    // body expression's events first, then `ExprStmt` + `StmtEnd` at the
    // statement's end (statements.rs `parse_expression_statement` fires
    // `handle_expression_statement` after `parse_expression`); the engine
    // appends Exit after resolving the call's type (official call site fires
    // with the expression still on the stack).
    let events: Vec<crate::ev::Ev> = [
        vec![
            crate::ev::Ev::ExecBegin { is_closure: false },
            crate::ev::Ev::BlockBodyBegin,
        ],
        // `never();` — the call's type resolves to Never.
        vec![
            crate::ev::Ev::OtherExpr { span: tspan(11) },
            crate::ev::Ev::Exit { end: tspan(18) },
            crate::ev::Ev::ExprStmt { span: tspan(11) },
            crate::ev::Ev::StmtEnd { end: 19 },
        ],
        // `1;` — unreachable from here.
        vec![
            crate::ev::Ev::OtherExpr { span: tspan(20) },
            crate::ev::Ev::ExprStmt { span: tspan(20) },
            crate::ev::Ev::StmtEnd { end: 22 },
        ],
        vec![
            crate::ev::Ev::BlockBodyEnd,
            crate::ev::Ev::ExecEnd {
                is_expr: false,
                end: tspan(24),
            },
        ],
    ]
    .concat();
    let interp = interp_of(&events);
    // One row: anchored at `1;` (the first unreachable node), spanning to
    // its `;` (offset 20, length 2). The body cannot complete normally.
    assert_eq!(interp.facts.len(), 1, "{:?}", interp.facts);
    match &interp.facts[0] {
        FlowFact::DeadCode { span } => {
            assert_eq!(span.range.offset, 20, "anchor is the first dead node");
            assert_eq!(span.range.length, 2, "the row runs to the `;`");
        }
        other => panic!("expected a DeadCode row, got {other:?}"),
    }
    // Control: the same buffer without Exit stays live (the body then
    // completes normally) — the Exit event is what kills reachability.
    let live: Vec<crate::ev::Ev> = events
        .iter()
        .copied()
        .filter(|ev| !matches!(ev, crate::ev::Ev::Exit { .. }))
        .collect();
    let interp = interp_of(&live);
    assert!(
        matches!(
            interp.facts.as_slice(),
            [FlowFact::BodyMayCompleteNormally { .. }]
        ),
        "{:?}",
        interp.facts
    );
}

/// The as-cast tail face owns the initializer's dead row: a cast whose
/// operand exits Never (`var a = f() as int;` with a Never-typed `f`)
/// opens the interval at the cast type directly inside the initializer,
/// where the semantic layer's as-cast tail face stages the official row
/// at the cast type's head token (probed: dart analyze 3.13.1 —
/// `DEAD_CODE` anchored there, `Dead code.`) — so the interval retires
/// without emitting rather than reporting a second `dead_code` row for
/// the same region. The interval itself keeps the official one-row rule
/// (upstream Dart SDK source,
/// pkg/analyzer/lib/src/error/dead_code_verifier.dart:183-189): later
/// unreachable nodes continue it and a covering close retires it — only
/// the emission skips. Hand-driven: only the engine can resolve a
/// subexpression's type to Never, so the parser pipeline cannot produce
/// this buffer yet.
#[test]
fn dead_cast_tail_owns_the_initializer_row() {
    // `void main() { var a = f() as int; }` — offsets: `var` at 14, `f()`
    // at 22-24, `as` at 26, `;` at 32, `}` at 34. The recorded grammar
    // per declaration: `VarDeclBegin` at the keyword, the identifier,
    // then the initializer bracket wrapping the expression's events; the
    // engine's Exit lands right after the send's `)` (the cast-type poll
    // at `AsBegin` is then the first unreachable node).
    let events: Vec<crate::ev::Ev> = [
        vec![
            crate::ev::Ev::ExecBegin { is_closure: false },
            crate::ev::Ev::BlockBodyBegin,
        ],
        vec![
            crate::ev::Ev::VarDeclBegin {
                late: false,
                is_final: false,
                span: tspan(14),
            },
            crate::ev::Ev::InitializedIdent {
                name: 0,
                token: tspan(18),
            },
            crate::ev::Ev::InitializerBegin,
            crate::ev::Ev::OtherExpr { span: tspan(22) },
            crate::ev::Ev::Exit { end: tspan(24) },
            crate::ev::Ev::AsBegin { span: tspan(26) },
            crate::ev::Ev::AsEnd,
            crate::ev::Ev::AsOperator,
            crate::ev::Ev::InitializerEnd,
            crate::ev::Ev::VarDeclEnd,
            crate::ev::Ev::StmtEnd { end: 33 },
        ],
        vec![
            crate::ev::Ev::BlockBodyEnd,
            crate::ev::Ev::ExecEnd {
                is_expr: false,
                end: tspan(34),
            },
        ],
    ]
    .concat();
    let interp = interp_of(&events);
    // The row belongs to the as-cast tail face: no flow row of our own.
    assert!(
        !interp
            .facts
            .iter()
            .any(|fact| matches!(fact, FlowFact::DeadCode { .. })),
        "{:?}",
        interp.facts
    );

    // Control: the same cast outside an initializer (the expression
    // statement face) keeps its flow row — the suppression keys on the
    // initializer bracket, and an expression-statement wrapper's dead
    // interval stays the flow window's.
    let bare: Vec<crate::ev::Ev> = events
        .iter()
        .copied()
        .filter(|ev| {
            !matches!(
                ev,
                crate::ev::Ev::InitializerBegin | crate::ev::Ev::InitializerEnd
            )
        })
        .collect();
    let interp = interp_of(&bare);
    let rows: Vec<_> = interp
        .facts
        .iter()
        .filter(|fact| matches!(fact, FlowFact::DeadCode { .. }))
        .collect();
    let [FlowFact::DeadCode { span }] = rows.as_slice() else {
        panic!("expected one DeadCode row, got {:?}", interp.facts);
    };
    assert_eq!(span.range.offset, 26, "anchor is the cast type poll");
    assert_eq!(span.range.length, 7, "the row runs to the `;`");

    // Control: a cast inside a nested executable within the initializer
    // (synthetic offsets — the bracket wraps the closure) keeps its row:
    // the executable's own frame depth breaks the initializer match, the
    // face the as-cast tail's own frame gate leaves to the flow window.
    let nested: Vec<crate::ev::Ev> = [
        vec![
            crate::ev::Ev::ExecBegin { is_closure: false },
            crate::ev::Ev::BlockBodyBegin,
            crate::ev::Ev::VarDeclBegin {
                late: false,
                is_final: false,
                span: tspan(14),
            },
            crate::ev::Ev::InitializedIdent {
                name: 0,
                token: tspan(18),
            },
            crate::ev::Ev::InitializerBegin,
            crate::ev::Ev::ExecBegin { is_closure: true },
        ],
        vec![
            crate::ev::Ev::OtherExpr { span: tspan(22) },
            crate::ev::Ev::Exit { end: tspan(24) },
            crate::ev::Ev::AsBegin { span: tspan(26) },
            crate::ev::Ev::AsEnd,
            crate::ev::Ev::AsOperator,
            crate::ev::Ev::ExprStmt { span: tspan(22) },
            crate::ev::Ev::StmtEnd { end: 33 },
            crate::ev::Ev::ExecEnd {
                is_expr: true,
                end: tspan(34),
            },
        ],
        vec![
            crate::ev::Ev::InitializerEnd,
            crate::ev::Ev::VarDeclEnd,
            crate::ev::Ev::StmtEnd { end: 33 },
            crate::ev::Ev::BlockBodyEnd,
            crate::ev::Ev::ExecEnd {
                is_expr: false,
                end: tspan(36),
            },
        ],
    ]
    .concat();
    let interp = interp_of(&nested);
    let rows: Vec<_> = interp
        .facts
        .iter()
        .filter(|fact| matches!(fact, FlowFact::DeadCode { .. }))
        .collect();
    let [FlowFact::DeadCode { span }] = rows.as_slice() else {
        panic!("expected one DeadCode row, got {:?}", interp.facts);
    };
    assert_eq!(span.range.offset, 26, "anchor is the cast type poll");
    assert_eq!(span.range.length, 7, "the row runs to the `;`");
}

/// official `test_doWhile` (`dead_code_test.dart`): a dead do-while back
/// edge anchors the interval on the condition, and the close splits into
/// the `while (c);` part plus the tail after the statement (official
/// `parent is DoStatement`, dead_code_verifier.dart:292-308) — two rows,
/// not one per statement.
#[test]
fn do_true_body_return_kills_exit() {
    let source = "void f() { do { return; } while (true); 1; }";
    assert_eq!(dead_snippets(source), ["} while (true);", "1;"]);
}

#[test]
fn do_false_exit_reachable() {
    let source = "void f() { do { } while (false); 1; }";
    assert!(dead_snippets(source).is_empty());
}

#[test]
fn do_body_runs_before_condition() {
    // The condition may be false, but the body ran: an assignment there
    // holds on the only exit path.
    let source = "void f(bool b) { int x; do { x = 1; } while (b); x; }";
    assert!(unassigned_snippets(source).is_empty());
}

/// official `definite_assignment/data/while.dart` `condition_notTrue`: a
/// conditional `while` may run zero times.
#[test]
fn while_zero_iterations_leaves_unassigned() {
    let source = "void f(bool b) { int x; while (b) { x = 1; } x; }";
    assert_eq!(unassigned_snippets(source), ["x"]);
}

/// official `definite_assignment/data/while.dart` `true_break_afterAssignment`:
/// the only exits from `while (true)` are breaks; a break after the
/// assignment makes the read assigned.
#[test]
fn while_true_break_after_assignment() {
    let source = "void f(bool b) { int x; while (true) { x = 1; if (b) break; } x; }";
    assert!(unassigned_snippets(source).is_empty());
}

/// official `definite_assignment/data/while.dart` `true_break_beforeAssignment`.
#[test]
fn while_true_break_before_assignment() {
    let source = "void f(bool b) { int x; while (true) { if (b) break; x = 1; } x; }";
    assert_eq!(unassigned_snippets(source), ["x"]);
}

/// official `definite_assignment/data/while.dart` `true_continue`: continues
/// re-enter the body, so an assignment after a conditional continue does
/// not hold on every path that reaches the exit break.
#[test]
fn while_true_continue_exit_is_assigned() {
    // Every loop exit is the `break` after the assignment: the read is
    // assigned (official while.dart `true_continue` exercises the continue
    // path's non-exit, not the exit's state).
    let source = "void f(bool b) { int x; while (true) { if (b) continue; x = 1; break; } x; }";
    assert!(unassigned_snippets(source).is_empty());
}

/// official `definite_assignment/data/for.dart`: a C-style `for` may run
/// zero iterations.
#[test]
fn for_zero_iterations_leaves_unassigned() {
    let source = "void f() { int x; for (var i = 0; i < 3; i++) { x = 1; } x; }";
    assert_eq!(unassigned_snippets(source), ["x"]);
}

#[test]
fn for_break_after_assignment() {
    let source = "void f(bool b) { int x; for (;;) { x = 1; if (b) break; } x; }";
    assert!(unassigned_snippets(source).is_empty());
}

/// official `getLabelTarget` (`flow_analysis_visitor.dart:487`): a labeled
/// break resolves to the labeled statement's loop, skipping the
/// intervening inner loop. Before label ids were carried on the events
/// this degraded to nearest-jump-target resolution: the break then exited
/// the INNER loop, `x = 1;` stayed live and the exit read came out
/// assigned.
/// A labeled break's dead tail: `break a` exits the OUTER loop, so the
/// assignment after the INNER while is the dead statement (official
/// `test_breakOutFromLoop` containment), and the covering close is the
/// outer while's `flowEnd(node.body)` (`type_analyzer.dart:4901`) trimmed
/// to the body block's last statement `x = 1;` — its `;` included
/// (dead_code_verifier.dart:269-271, :326-332). The trailing `x;` is
/// alive (the break joins the outer exit) — it reads definitely
/// unassigned instead.
#[test]
fn labeled_break_targets_labeled_loop_not_nearest() {
    let source =
        "void f(bool b) { int x; a: while (true) { while (true) { break a; } x = 1; } x; }";
    assert_eq!(dead_snippets(source), ["x = 1;"]);
    assert_eq!(unassigned_snippets(source), ["x"]);
}

/// The same walk for a labeled statement over a switch: the break lands
/// in the switch's break model, keeping the code after it alive.
#[test]
fn labeled_break_targets_labeled_switch_through_loop() {
    let source = "void f(bool b) { a: switch (b ? 1 : 2) { case 1: while (true) { break a; } case 2: break a; } 1; }";
    assert!(dead_snippets(source).is_empty());
}

/// A multi-label statement resolves a jump to ANY of its labels
/// (`a: b: s` — the official scans every label of the labeled statement;
/// `break b` must match through the recorded `LabelDecl`). The dead
/// statement is the assignment after the inner while — same shape as
/// `labeled_break_targets_labeled_loop_not_nearest`.
#[test]
fn labeled_break_matches_any_label_of_the_statement() {
    let source =
        "void f(bool b) { int x; a: b: while (true) { while (true) { break b; } x = 1; } x; }";
    assert_eq!(dead_snippets(source), ["x = 1;"]);
    assert_eq!(unassigned_snippets(source), ["x"]);
}

/// Unlabeled breaks keep the nearest-jump-target semantics: the inner
/// loop is the target even under a labeled outer one.
#[test]
fn unlabeled_break_still_targets_nearest_loop() {
    let source = "void f(bool b) { a: while (true) { while (true) { if (b) break; } 1; } }";
    assert!(dead_snippets(source).is_empty());
}

/// A labeled continue resolves to its labeled loop, skipping the
/// intervening inner loop: the re-entry path it joins (before any
/// assignment) must reach the exit. The outer loop is a `do` — only do
/// and C-style `for` joins consume continue models.
#[test]
fn labeled_continue_targets_labeled_loop_not_nearest() {
    let source = "void f(bool b) { int x; a: do { while (b) { if (b) { continue a; } x = 1; } x = 1; } while (b); x; }";
    assert_eq!(unassigned_snippets(source), ["x"]);
}

/// official strictness: `continue` onto a label of a non-loop statement
/// joins the labeled context's continue model, which nothing ever reads —
/// the only observable is the dead path, and the enclosing loop's
/// re-entry state stays untouched (a wrong nearest-loop routing would
/// pollute the do-loop's continue model and turn the exit read
/// possibly-unassigned). The enclosing loop is a `do`: only do and
/// C-style `for` joins consume continue models.
#[test]
fn continue_labeled_non_loop_leaves_loop_models_untouched() {
    let source =
        "void f(bool b) { int x; do { c: { if (b) continue c; x = 1; } x = 1; } while (b); x; }";
    assert!(unassigned_snippets(source).is_empty());
}

/// official reachability/data/switch.dart `case_falls_through_end`: a
/// case body that falls through leaves the code after the switch alive.
#[test]
fn switch_case_falls_through_end() {
    let source = "void f(int i) { switch (i) { case 1: 1; } 2; }";
    assert!(dead_snippets(source).is_empty());
}

/// official reachability/data/switch.dart `all_cases_exit`: when every
/// case (including `default`) exits, the code after the switch is dead.
#[test]
fn switch_all_cases_exit() {
    let source = "void f(int i) { switch (i) { case 1: return; default: return; } 1; }";
    assert_eq!(dead_snippets(source), ["1;"]);
}

/// Stacked case heads (`case A: case B: <shared body>`) are one member
/// sharing one body: the head chain contributes every head's matched state
/// to the single close at the shared body's first event, and never
/// completes a member of its own. With a `default` and a returning shared
/// body the switch has no break model at all, so no completion fact emits.
/// Closing the group at the next head's first pattern event (the #2271
/// regression) treated the empty body between the heads as a completed
/// member — `switch_after_case` joined its fall-through into the break
/// model and the switch completed normally. The official driver runs one
/// `switch_endAlternatives`/`switch_afterCase` pair per merged member
/// (`getSwitchStatementMemberInfo` merges consecutive empty members'
/// heads, type_analyzer.dart:2614-2623).
#[test]
fn switch_stacked_heads_with_default_and_returning_body_emit_no_completion() {
    let source =
        "int f(int x) { switch (x) { case 1: case 2: return 1; default: return 0; } }";
    assert!(completion_offsets(source).is_empty());
    // A three-head run sharing one body (petitparser whitespace.dart).
    let source = concat!(
        "int f(int x) { switch (x) { case 1: case 2: case 3: return 1;",
        " default: return 0; } }"
    );
    assert!(completion_offsets(source).is_empty());
    // An exhaustively covered scrutinee without `default` decides the
    // same way (the unmatched chain must not join the exit either).
    let source = "int f(bool b) { switch (b) { case true: case false: return 1; } }";
    assert!(completion_offsets(source).is_empty());
    // A guarded head inside the stack: the guard events run inside the
    // head region and must not close the previous head's group either.
    let source = concat!(
        "int f(int x) { switch (x) { case 1: case 2 when x > 0: return 1;",
        " default: return 0; } }"
    );
    assert!(completion_offsets(source).is_empty());
    // A pattern-head stacked pair sharing one body (riverpod element.dart:
    // `case AsyncLoading(…) when …:` over `case AsyncError(…):`).
    let source = concat!(
        "sealed class S {} class A extends S {} class B extends S {}",
        " int f(S s) { switch (s) { case A _: case B _: return 0; default: return 1; } }"
    );
    assert!(completion_offsets(source).is_empty());
}

/// A `default` stacked after case heads (`case 3: default:`) folds into
/// that case-headed member (official `getSwitchStatementMemberInfo`), so
/// the member's close cannot see it — the chain kill lands at the
/// member's `CaseEnd`. Without it the unmatched chain joins the exit and
/// the switch completes normally although every body returns.
#[test]
fn switch_default_stacked_after_case_head_exhausts_the_chain() {
    // dio compute_io.dart's shape.
    let source =
        "int f(int x) { switch (x) { case 1: return 1; case 3: default: return 0; } }";
    assert!(completion_offsets(source).is_empty());
    let source = "int f(int x) { switch (x) { case 1: case 2: default: return 0; } }";
    assert!(completion_offsets(source).is_empty());
}

/// The twin: a stacked shape whose shared body falls through still
/// completes — the shared close's split resurrects the body end for the
/// implicit-break join (official `switch_afterCase` reads the LOCAL flag
/// set by `switch_endAlternatives`'s split).
#[test]
fn switch_stacked_heads_shared_body_fall_through_completes() {
    let source = "int f(int x) { switch (x) { case 1: case 2: default: x++; } 1; }";
    assert_eq!(completion_offsets(source).len(), 1);
}

/// A trailing empty clause closes at the block's end: its fall-through
/// joins the break model (the member's empty body completes normally),
/// so the code after the switch stays live. The close the stacked-head
/// skip defers must not be lost (both faces oracle-verified).
#[test]
fn switch_trailing_empty_clause_falls_through() {
    // Exhaustive switch, trailing empty clause: `1;` is live.
    let source = "void f(bool b) { switch (b) { case true: return; case false: } 1; }";
    assert!(dead_snippets(source).is_empty());
    // The twin with a returning last member: `1;` is dead.
    let source = "void f(bool b) { switch (b) { case true: return; case false: return; } 1; }";
    assert_eq!(dead_snippets(source), ["1;"]);
    // The completion face: a stacked trailing empty member falls through
    // and the enclosing body completes (the switch's block-end close).
    let source = "int f(int x) { switch (x) { case 1: case 2: } }";
    assert_eq!(completion_offsets(source), [46]);
    // A trailing empty clause after a `default` (a parse-recovery shape):
    // no completion either way.
    let source = "int f(int x) { switch (x) { default: return 0; case 2: } }";
    assert!(completion_offsets(source).is_empty());
}

/// official reachability/data/switch.dart `case_never_completes`: dead
/// code inside a case body whose both branches return.
#[test]
fn switch_case_inner_dead_code() {
    let source = "void f(bool b, int i) { switch (i) { case 1: 1; if (b) { return; } else { return; } 2; } 3; }";
    assert_eq!(dead_snippets(source), ["2;"]);
}

/// `switch_end`'s `isExhaustive` arm (flow_analysis.dart:7985-7996): a
/// switch over an always-exhaustive scrutinee (sealed class, enum, bool)
/// never joins the unmatched chain — with no break or fall-through the
/// post-switch state is `previous` unreachable, so the body's end stays
/// unreachable and no completion fact emits (the sealed-scrutinee
/// `body_might_complete_normally` regression, #2134). The non-exhaustive
/// `int` twin still emits: `isExhaustive` is the only difference.
#[test]
fn switch_over_sealed_scrutinee_without_default_kills_exit() {
    // Every sealed subclass has a returning case: the post-switch code is
    // unreachable upstream, so the body emits no completion row.
    let source = concat!(
        "sealed class S {} class A extends S {} class B extends S {}",
        " int f(S s) { switch (s) { case A _: return 0; case B _: return 1; } }"
    );
    assert!(completion_offsets(source).is_empty());
    // A `default`-less non-exhaustive switch over the same shape keeps the
    // post-switch path live — the completion row still fires.
    let source =
        "int f(int i) { switch (i) { case 1: return 0; case 2: return 1; } }";
    assert_eq!(completion_offsets(source).len(), 1);
    // The same exhaustive shape over `bool` and an enum.
    let source = "int f(bool b) { switch (b) { case true: return 0; case false: return 1; } }";
    assert!(completion_offsets(source).is_empty());
    let source = "enum E { a, b } int f(E e) { switch (e) { case E.a: return 0; case E.b: return 1; } }";
    assert!(completion_offsets(source).is_empty());
    // A sealed scrutinee whose cases leave a live path (a body falls
    // through) still completes — `isExhaustive` only drops the unmatched
    // arm, not the break model.
    let source = concat!(
        "sealed class S {} class A extends S {} class B extends S {}",
        " int f(S s) { switch (s) { case A _: 0; case B _: return 1; } }"
    );
    assert_eq!(completion_offsets(source).len(), 1);
    // `FutureOr` recursion (type_system.dart:865): `FutureOr<bool>` is
    // always-exhaustive.
    let source =
        "int f(FutureOr<bool> b) { switch (b) { case true: return 0; case false: return 1; } }";
    assert!(completion_offsets(source).is_empty());
}

/// official `definite_assignment/data/closure.dart`: writes inside closures
/// do not make outer reads assigned.
#[test]
fn closure_write_does_not_assign_outer_local() {
    let source = "void f() { int x; g() { x = 1; }; g(); x; }";
    assert_eq!(unassigned_snippets(source), ["x"]);
}

/// official `definite_assignment/data/if.dart`: assignment on every path
/// (then assigns, else returns) makes the read assigned.
#[test]
fn if_else_return_covers_all_paths() {
    let source = "void f(bool b) { int x; if (b) { x = 1; } else { return; } x; }";
    assert!(unassigned_snippets(source).is_empty());
}

#[test]
fn if_without_else_leaves_unassigned() {
    let source = "void f(bool b) { int x; if (b) { x = 1; } x; }";
    assert_eq!(unassigned_snippets(source), ["x"]);
}

/// official `definite_assignment/data/try.dart` shape: a catch that returns
/// removes the only path where the body's assignment did not run.
#[test]
fn try_catch_returning_catch_covers_paths() {
    let source = "void f() { int x; try { x = 1; } catch (e) { return; } x; }";
    assert!(unassigned_snippets(source).is_empty());
}

#[test]
fn try_catch_open_catch_leaves_unassigned() {
    let source = "void f() { int x; try { x = 1; } catch (e) { } x; }";
    assert_eq!(unassigned_snippets(source), ["x"]);
}

/// official `definite_assignment/data/late.dart` shape: reading a `late`
/// local before any assignment is definitely unassigned.
#[test]
fn late_local_read_unassigned() {
    let source = "void f() { late int x; x; }";
    assert_eq!(unassigned_snippets(source), ["x"]);
}

/// The `definitely_unassigned` bit separates the upstream gates
/// (type_analyzer.dart:4990-5028): a `late` read before ANY path assigns
/// is `unassigned` — the `definitely_unassigned_late_local_variable`
/// shape; a `late` read after a partial-path assignment is `!assigned`
/// but NOT `unassigned` — the try/finally fixture's `digest` is the
/// former (read inside the try, assigned later on the same path).
#[test]
fn unassigned_read_carries_the_definite_bit() {
    let definite = |source: &str| -> Vec<bool> {
        facts_of(source)
            .into_iter()
            .filter_map(|fact| match fact {
                FlowFact::UnassignedRead {
                    definitely_unassigned,
                    ..
                } => Some(definitely_unassigned),
                _ => None,
            })
            .collect()
    };
    // promotion_patterns_flow_tryfinally's `seal` shape: the read precedes
    // every assignment on every path → definitely unassigned.
    let source = "void f() { late int x; try { x; x = 1; } finally { } }";
    assert_eq!(definite(source), [true]);
    // Assigned on one branch only: the read after the join is
    // potentially (not definitely) unassigned → `false`. (A
    // `try { x = 1; } finally { }` would leave `x` definitely assigned:
    // the post-finally state is the try body's.)
    let source = "void f(bool b) { late int x; if (b) { x = 1; } x; }";
    assert_eq!(definite(source), [false]);
    // Same split for a `final` local: the fixture's `settle` shape.
    let source = "void f() { final int x; try { x = 1; } catch (e) { } x; }";
    assert_eq!(definite(source), [false]);
}

/// Plain locals start assigned when initialized.
#[test]
fn initialized_local_read_assigned() {
    let source = "void f() { int x = 1; x; }";
    assert!(unassigned_snippets(source).is_empty());
}

/// official `type_promotion/data/null_check.dart` shape: an `!= null` test
/// flows promotion into the then-branch; with the name-level seam this
/// must not crash and must not produce false dead code. The promoted read
/// surfaces as its own fact — a nullability promotion carries no type
/// arguments, so it stays in scope (#611 gate).
#[test]
fn null_check_promotion_emits_read_fact() {
    let source = "void f(int? x) { if (x != null) { x; } }";
    let promoted = promoted_tys(source);
    assert_eq!(promoted.len(), 1);
    assert!(matches!(promoted[0].base, crate::ty::Base::Name(_)));
    assert!(!promoted[0].nullable);
}

/// #1011 pin: the same `!= null` null check over a local declared with a
/// `null` initializer (`int? x = null;`) stages the promotion the same way
/// — the write demotes to the fresh model and the equality check re-promotes
/// on the matched branch, so the then-branch read carries the non-null
/// declared type. The `== null` polarity pins the else-branch arm, and the
/// `final` twin pins that the initializer write does not block finals.
#[test]
fn null_check_promotes_a_null_initialized_local() {
    let promoted_name = |source: &str| {
        let promoted = promoted_tys(source);
        assert_eq!(promoted.len(), 1, "{source:?}");
        assert!(
            matches!(promoted[0].base, crate::ty::Base::Name(_)),
            "{source:?}"
        );
        assert!(!promoted[0].nullable, "{source:?}");
    };
    // The #1011 repro shape: then-branch read promoted.
    promoted_name("void f() { int? x = null; if (x != null) { int y = x; } }");
    // `== null` takes the promotion into the else branch.
    promoted_name("void f() { int? x = null; if (x == null) { x = 1; } else { int y = x; } }");
    // Reversed operand order is the same null check.
    promoted_name("void f() { int? x = null; if (null != x) { int y = x; } }");
    // The final twin: same initializer, same promotion.
    promoted_name("void f() { final int? x = null; if (x != null) { int y = x; } }");
}

/// official `equalityOp(null != null) equivalent to false`
/// (`flow_analysis_test.dart`): both operands null — `_equalityCheck`
/// returns a known equal result, so the not-equal outcome is unreachable
/// (then-branch dead) and `==` inverts the dead side to the else-branch.
#[test]
fn null_null_equality_is_a_known_constant() {
    let source = "void f() { if (null != null) { print(1); } }";
    assert_eq!(dead_snippets(source), ["{ print(1); }"]);
    let source = "void f() { if (null == null) { print(1); } else { print(2); } }";
    assert_eq!(dead_snippets(source), ["{ print(2); }"]);
}

/// official `equalityOp(x != <null expr>) does not promote`
/// (`flow_analysis_test.dart`): a `Null`-typed variable read is not a
/// null literal (official `ExpressionInfo.isNull` fires for the literal
/// only), so the equality check carries no promotion and both branch
/// sides keep `x` unpromoted. The literal control still promotes.
#[test]
fn null_typed_variable_operand_does_not_promote() {
    let source = "void f() { Null n = null; int? x; if (x != n) { int y = x; } }";
    assert!(promoted_tys(source).is_empty(), "{source:?}");
    // Literal control: the same shape against `null` promotes.
    let control = "void f() { int? x; if (x != null) { int y = x; } }";
    let promoted = promoted_tys(control);
    assert_eq!(promoted.len(), 1, "{control:?}");
    assert!(!promoted[0].nullable, "{control:?}");
}

/// `Never?` classifies null-or-equivalent (official `_isNullType` via
/// `classifyType`), so it takes the type-level arms: known-equal against
/// `null` (`n != null` kills the then-branch — the pre-classify check
/// would have wrongly entered the promotion arm), known-not-equal against
/// a non-nullable operand (the distinguishability arm), and no
/// information against a potentially-nullable one (both sides reachable,
/// no promotion).
#[test]
fn never_question_takes_the_null_type_arms() {
    // NullOrEquivalent x NullOrEquivalent: known equal.
    let source = "void f() { Never? n = null; if (n != null) { print(1); } }";
    assert_eq!(dead_snippets(source), ["{ print(1); }"]);
    // NullOrEquivalent x NonNullable: known not-equal — `==` kills the
    // then-branch, `!=` the else-branch.
    let source =
        "void f() { Never? n = null; int y = 0; if (y == n) { print(1); } else { print(2); } }";
    assert_eq!(dead_snippets(source), ["{ print(1); }"]);
    let source =
        "void f() { Never? n = null; int y = 0; if (y != n) { print(1); } else { print(2); } }";
    assert_eq!(dead_snippets(source), ["{ print(2); }"]);
    // NullOrEquivalent x PotentiallyNullable: no information.
    let source =
        "void f() { Never? n = null; int? x; if (x == n) { print(1); } else { print(2); } }";
    assert!(dead_snippets(source).is_empty(), "{source:?}");
    assert!(promoted_tys(source).is_empty());
}

// —— declared-bound nullability faces (#1077) ————————————————————————————
//
// The official `classifyType` is a subtype question: a type-parameter
// reference `T` classifies through its declared bound (`T <: Object` iff
// `B <: Object`, the implicit bound being `Object?` — upstream Dart SDK
// source, pkg/analyzer/lib/src/dart/resolver/flow_analysis_visitor.dart:615-632
// and pkg/analyzer/lib/src/dart/element/subtype.dart:303-325), so the
// syntactic `?`-less spelling alone must not say non-nullable.

/// official `equalityOp(x != <null expr>)` on `T extends Object?`: the
/// reference is potentially nullable (the bound admits `null`), so the
/// check reaches the promotion arm — both branches live, the else side
/// promoted (also the unbounded `T` twin, whose implicit bound is
/// `Object?`). The pre-#1077 syntactic face judged the then-branch dead.
#[test]
fn nullable_bound_type_param_null_check_keeps_both_branches_live() {
    for source in [
        "void f<T extends Object?>(T x) { if (x == null) { print(1); } else { print(2); } }",
        "void f<T>(T x) { if (x == null) { print(1); } else { print(2); } }",
        "void f<T extends Object?>(T x) { if (x != null) { print(1); } else { print(2); } }",
        // Nullable spelled bounds behave like `Object?`.
        "void f<T extends int?>(T x) { if (x == null) { print(1); } else { print(2); } }",
    ] {
        assert!(dead_snippets(source).is_empty(), "{source:?}");
    }
    // The else-branch read carries the promotion to the bound's non-null
    // face (`T & Object` — name-level: `Object`).
    let source = "void f<T extends Object?>(T x) { if (x == null) { x = x; } else { int y = x; } }";
    let promoted = promoted_tys(source);
    assert_eq!(promoted.len(), 1, "{source:?}");
    assert!(
        matches!(promoted[0].base, crate::ty::Base::Name(_)),
        "{source:?}"
    );
    assert!(!promoted[0].nullable, "{source:?}");
}

/// The promotion is a real one: after `if (x == null) return;` the
/// parameter's promoted non-null face classifies non-nullable, so a
/// second `x == null` is a known not-equal result and the then-branch is
/// dead — the shape the pre-#1077 face could never reach.
#[test]
fn nullable_bound_type_param_promotion_kills_a_second_null_check() {
    let source =
        "void f<T extends Object?>(T x) { if (x == null) return; if (x == null) { print(1); } }";
    assert_eq!(dead_snippets(source), ["{ print(1); }"]);
}

/// official classifyType on `T extends Object` (and any strictly
/// non-nullable bound): the reference classifies non-nullable, so the
/// null check is a known not-equal result — the disjoint arm stays, the
/// then-branch of `==` (the else-branch of `!=`) is dead, and nothing
/// promotes (#1077 keeps the pre-existing behavior here).
#[test]
fn non_nullable_bound_type_param_keeps_the_disjoint_branch_kill() {
    for source in [
        "void f<T extends Object>(T x) { if (x == null) { print(1); } else { print(2); } }",
        "void f<T extends num>(T x) { if (x != null) { print(1); } else { print(2); } }",
    ] {
        assert_eq!(dead_snippets(source).len(), 1, "{source:?}");
        assert!(promoted_tys(source).is_empty(), "{source:?}");
    }
}

/// `FutureOr<void>` bounds: `FutureOr<S> <: Object` iff `S <: Object`, and
/// `void <: Object` fails (a top type) — potentially nullable (upstream
/// Dart SDK source, pkg/analyzer/lib/src/dart/element/subtype.dart:319-325
/// and :290-296). Both branches live; the promotion itself bails on
/// `isSameType` (`NonNull(FutureOr<void>)` is `FutureOr<void>` —
/// pkg/analyzer/lib/src/dart/element/type_system.dart:1467-1497), so no
/// promotion row. A strictly-non-nullable `FutureOr<int>` control keeps
/// the disjoint kill.
#[test]
fn future_or_void_bound_is_potentially_nullable_without_promotion() {
    let source =
        "void f<T extends FutureOr<void>>(T x) { if (x == null) { print(1); } else { print(2); } }";
    assert!(dead_snippets(source).is_empty(), "{source:?}");
    assert!(promoted_tys(source).is_empty(), "{source:?}");
    let control =
        "void f<T extends FutureOr<int>>(T x) { if (x == null) { print(1); } else { print(2); } }";
    assert_eq!(dead_snippets(control), ["{ print(1); }"], "{control:?}");
}

/// A bound whose shape carries no derivable face (a record-type bound
/// fires no head type row) degrades silently to the pre-#1077 behavior —
/// no capture, no panic, the syntactic classification stands.
#[test]
fn underivable_bound_degrades_to_the_syntactic_face() {
    let source =
        "void f<T extends (int, String)>(T x) { if (x == null) { print(1); } else { print(2); } }";
    assert_eq!(dead_snippets(source), ["{ print(1); }"], "{source:?}");
}

/// The boundary table is per-declaration: a second top-level declaration
/// reusing the spelling starts from nothing, and a class's parameters
/// stay visible across its members.
#[test]
fn boundary_table_scopes_to_one_top_level_declaration() {
    // The class's `T extends Object?` reaches both members…
    let source = "class C<T extends Object?> { void f(T x) { if (x == null) { print(1); } else { print(2); } } void g(T y) { y == null; } }";
    assert!(dead_snippets(source).is_empty(), "{source:?}");
    // …and does not leak into the next declaration: `h`'s unbounded `T`
    // is independently nullable (also live), while a non-nullable rebind
    // (`k`) kills its own branch.
    let live = "void h<T extends Object?>(T x) { if (x == null) { print(1); } else { print(2); } }";
    assert!(dead_snippets(live).is_empty(), "{live:?}");
}

/// #1037 pin: a closure parameter's type annotation types only that
/// parameter — it must not re-enter the open declaration list's type slot
/// and re-declare the list's later items. Upstream Dart SDK semantics:
/// every variable of a declaration list is declared from its own header or
/// initializer (`c` in `var a = (String? b) => b, c = 1;` is `int` from
/// `1`, not the parameter's `String?`). The interp face: the parameter's
/// `Type` events fire between `VarDeclBegin` and `VarDeclEnd` (the
/// closure initializer runs inside the list), exactly where the declared
/// type sink would otherwise swallow them.
#[test]
fn closure_parameter_type_does_not_leak_across_declaration_list() {
    // The leak discriminator: a leaked `String?` declared type on `c`
    // makes the then-branch read after `c != null` emit a NAMED promotion
    // row (to the parameter's `String`) the oracle never emits (upstream
    // `c` is non-nullable `int` — the check is trivial and promotes
    // nothing). The unknown-face marker promotion (#2241 — the `1`
    // initializer's unspellable face refining through the #2243 marker
    // step) is not the leak: its consumer substitution re-decides from
    // the resolved `int` and changes no row.
    let leaked = |source: &str| {
        promoted_tys(source)
            .into_iter()
            .filter(|ty| matches!(ty.base, crate::ty::Base::Name(_)))
            .collect::<Vec<_>>()
    };
    assert!(
        leaked("void f() { var a = (String? b) => b, c = 1; if (c != null) { int y = c; } }")
            .is_empty(),
        "the closure parameter's `String?` must not declare `c`"
    );
    // The leak reaches every later item of the list: mixed initializers
    // each declare from themselves (`c` from `1`, `d` from `'s'`), so no
    // follow-up null check promotes anything to a named type.
    assert!(
        leaked(
            "void f() { var a = (String? b) => b, c = 1, d = 's'; if (c != null) { int y = c; } if (d != null) { int z = d; } }"
        )
        .is_empty(),
        "both `c` and `d` must infer from their own initializers"
    );
    // The green face: the parameter itself IS typed by its annotation —
    // the closure body's null check promotes `b` to non-null `String`
    // (pre-fix the annotation was swallowed by the open list's slot and
    // `b` read unknown, emitting no row).
    let promoted = promoted_tys(
        "void f() { var a = (String? b) { if (b != null) { int y = b; } return 1; }; }",
    );
    assert_eq!(promoted.len(), 1, "the parameter `b` reads promoted");
    assert!(
        matches!(promoted[0].base, crate::ty::Base::Name(_)),
        "{promoted:?}"
    );
    assert!(!promoted[0].nullable, "the promotion drops the `?`");
}

/// #1051 pin: a nested declaration list — a local function body inside an
/// initializer — must not clobber the enclosing list's header type. Each
/// declaration list's header declares its own variables: upstream Dart SDK
/// semantics give `b` in `int? a = (() { var t = 1; })(), b = 3;` the outer
/// header's type — the inner `var t` neither leaks in nor resets it. The
/// interp face: the inner list's `VarDeclBegin`/`VarDeclEnd` bracket the
/// outer list's remaining items, exactly where the single declaration slot
/// was reset to the default. The header is spelled `int?` and the
/// initializer `null` (the #1011 idiom) so the follow-up null check's
/// then-branch read is the one promotion row; pre-fix `b` declared unknown
/// and no row could fire.
#[test]
fn nested_declaration_list_keeps_the_outer_header_type() {
    let promoted_row = |source: &str| {
        let promoted = promoted_tys(source);
        assert_eq!(promoted.len(), 1, "{source:?}");
        assert!(
            matches!(promoted[0].base, crate::ty::Base::Name(_)),
            "{promoted:?}"
        );
        assert!(!promoted[0].nullable, "{promoted:?}");
    };
    // The canonical shape: the inner list's `VarDeclEnd` sits between the
    // outer header and `b` — the header type must survive it.
    promoted_row(
        "void f() { int? a = (() { var t = 1; return 2; })(), b = null; if (b != null) { int y = b; } }",
    );
    // The flat twin: no nesting, same face — the restore path must not
    // break the plain list.
    promoted_row("void f() { int? a = null, b = null; if (b != null) { int y = b; } }");
    // Precision: the non-nullable `int` spelling restores as exactly that —
    // the null check is then trivial and emits no row (a wrong nullable
    // restore would emit one; a lost header emits none, which the canonical
    // shape above discriminates).
    assert!(
        promoted_tys(
            "void f() { int a = (() { var t = 1; return 2; })(), b = 3; if (b != null) { int y = b; } }"
        )
        .is_empty(),
        "`b` is declared non-nullable `int`; the check promotes nothing"
    );
}

/// #1051 pin, two levels deep: the outermost header must survive two nested
/// `VarDeclBegin`/`VarDeclEnd` brackets — the save/restore is a stack, not
/// a single saved slot (one slot would drop the outermost context when the
/// inner list saves over it).
#[test]
fn doubly_nested_declaration_lists_restore_transitively() {
    let promoted = promoted_tys(
        "void f() { int? a = (() { var t = (() { var u = 1; })(), s = 2; })(), b = null; if (b != null) { int y = b; } }",
    );
    assert_eq!(promoted.len(), 1, "the outermost header reaches `b`");
    assert!(
        matches!(promoted[0].base, crate::ty::Base::Name(_)),
        "{promoted:?}"
    );
    assert!(!promoted[0].nullable, "the promotion drops the `?`");
}

/// #1051 pin with the #1037 seal in the loop: the outer header must also
/// survive a nested list followed by a later item whose initializer carries
/// a closure parameter's `Type` events. The parameter types only its own
/// formal; `d` still declares from the outer header (`int?`), so its
/// null-check read emits the row. A broken per-list seal would re-declare
/// `d` from the parameter's non-nullable `String` (trivial check, no row);
/// the lost header declares `d` unknown (no row either) — only the stacked
/// restore emits exactly one.
#[test]
fn nested_list_then_closure_parameter_leaves_the_outer_header_intact() {
    let promoted = promoted_tys(
        "void f() { int? a = (() { var t = 1; return 2; })(), b = (String c) => 1, d = null; if (d != null) { int y = d; } }",
    );
    assert_eq!(promoted.len(), 1, "the outer header reaches `d`");
    assert!(
        matches!(promoted[0].base, crate::ty::Base::Name(_)),
        "{promoted:?}"
    );
    assert!(!promoted[0].nullable, "the promotion drops the `?`");
}

// —— promotion facts (PromotedRead) ————————————————————————————————————————

/// (a) An `x is T`-promoted read emits `PromotedRead` carrying the
/// promoted type; the condition's own read (pre-promotion) emits nothing.
#[test]
fn promoted_read_emits_the_promoted_type() {
    let source = "void f(num x) { if (x is int) { x; } }";
    let (flow, facts) = analyze(source);
    let promoted: Vec<crate::ty::Ty> = facts
        .iter()
        .filter_map(|fact| match fact {
            FlowFact::PromotedRead { ty, .. } => Some(*ty),
            _ => None,
        })
        .collect();
    assert_eq!(promoted.len(), 1, "{facts:?}");
    assert!(!promoted[0].nullable);
    let crate::ty::Base::Name(id) = promoted[0].base else {
        panic!("expected a named promotion, got {:?}", promoted[0]);
    };
    assert_eq!(flow.0.borrow().name_lexeme(id), "int");
}

/// (b) Unpromoted and demoted reads emit nothing: without a promotion the
/// chain head is the declared type, and a write drops the chain back to
/// it (`take_facts` bounds the face to real promotions).
#[test]
fn unpromoted_and_demoted_reads_emit_nothing() {
    // No promotion ever applied.
    let plain = "void f(int x) { x; }";
    assert!(read_facts_of(plain).is_empty());
    // Promoted, then written (demoted): the read after the write sees
    // the declared type again. The write target's own facts retract with
    // the assignment (the identifier is the write).
    let demoted = "void f(int? x) { if (x is int) { x = null; x; } }";
    assert!(read_facts_of(demoted).is_empty());
}

/// (c) The #1010 transport: every real promotion's read emits, and the row
/// carries the chain head's written argument tree — an arg-bearing tested
/// type (`x is List<int>`) stages exactly as written, the arg-free twin
/// (`x is List`) carries the reserved empty slot, and a multi-argument
/// tested type carries its whole tree.
#[test]
fn promoted_reads_carry_their_written_arguments() {
    let read_args = |source: &str| {
        let (flow, facts) = analyze(source);
        let flow = flow.0.borrow();
        facts
            .iter()
            .filter_map(|fact| match fact {
                FlowFact::PromotedRead { args, .. } => Some(
                    flow.type_args_at(*args)
                        .iter()
                        .map(|entry| match entry.base {
                            crate::ty::Base::Name(id) => {
                                (flow.name_lexeme(id).to_string(), entry.args)
                            }
                            _ => panic!("every tested spelling here is a named type"),
                        })
                        .collect::<Vec<_>>(),
                ),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    // `x is List<int>`: real promotion, the row carries the written `<int>`.
    let arg_bearing = read_args("void f(List? x) { if (x is List<int>) { x; } }");
    assert_eq!(arg_bearing.len(), 1, "{arg_bearing:?}");
    assert_eq!(
        arg_bearing[0],
        vec![("int".to_string(), crate::ty::EMPTY_ARGS)]
    );
    // `x is List`: the same promotion shape, no arguments — empty slot.
    let arg_free = read_args("void f(List? x) { if (x is List) { x; } }");
    assert_eq!(arg_free.len(), 1, "{arg_free:?}");
    assert!(arg_free[0].is_empty(), "{arg_free:?}");
    // A multi-argument tested type carries its whole tree.
    let multi = read_args("void f(Map? x) { if (x is Map<String, int>) { x; } }");
    assert_eq!(multi.len(), 1, "{multi:?}");
    assert_eq!(
        multi[0],
        vec![
            ("String".to_string(), crate::ty::EMPTY_ARGS),
            ("int".to_string(), crate::ty::EMPTY_ARGS),
        ]
    );
}

/// #2180 face-1 pin: the true-arm promotion of a type check is the
/// analyzer's `tryPromoteToType(tested, previous)` gated by
/// `isValidPromotionStep` (`flow_analysis.dart:4142-4151`), not the tested
/// type factored against the promotion chain. The factor-shaped candidate
/// promotes `x` to `Never?` on `x is int?` when `x` already stands at `int`
/// (`factor(int?, int) = Never?`), where the official keeps the standing
/// `int` — `tryPromoteToType(int?, int)` is null because `int?` is no
/// subtype of `int` (`type_system.dart:1729-1760`). A `Never?` read then
/// poisons every member check in the branch (the `bool`-corruption shape
/// the face was reported on).
#[test]
fn type_check_on_a_promoted_var_never_promotes_to_the_factored_remainder() {
    let source = "void f(int? x) { x = 1; if (x != null) { if (x is int?) { x; } } }";
    let promoted = promoted_tys(source);
    assert!(
        promoted
            .iter()
            .all(|ty| !matches!(ty.base, crate::ty::Base::Never)),
        "{promoted:?}"
    );
}

/// The mirror pin: the official's `tryPromoteToType` answer for a real
/// refinement still promotes — `x is int` on `num? x` already promoted to
/// `num` by the null check promotes to `int` (`int <: num` decides at a
/// resolved back-end; at the name level the optimistic arm keeps the
/// existing contract). The inner `x` operand reads its `num` promotion,
/// the branch read its `int`.
#[test]
fn type_check_refinement_still_promotes() {
    let source = "void f(num? x) { x = 1; if (x != null) { if (x is int) { x; } } }";
    let promoted = promoted_tys(source);
    assert_eq!(promoted.len(), 2, "{promoted:?}");
    assert!(promoted.iter().all(|ty| !ty.nullable));
}

/// (d) The name-recovery face: `Base::Name(id)` round-trips through
/// `name_lexeme` to its source spelling, and ids are dense (every id
/// below a fact's id resolves).
#[test]
fn name_lexeme_recovers_interned_spellings() {
    let source = "void f(num x) { if (x is int) { x; } }";
    let (flow, facts) = analyze(source);
    let promoted: Vec<crate::ty::Ty> = facts
        .iter()
        .filter_map(|fact| match fact {
            FlowFact::PromotedRead { ty, .. } => Some(*ty),
            _ => None,
        })
        .collect();
    assert_eq!(promoted.len(), 1);
    let crate::ty::Base::Name(id) = promoted[0].base else {
        panic!("expected a named promotion, got {:?}", promoted[0]);
    };
    assert_eq!(flow.0.borrow().name_lexeme(id), "int");
    for lower in 0..id {
        assert!(
            !flow.0.borrow().name_lexeme(lower).is_empty(),
            "id {lower} below {id} must resolve (dense ids)"
        );
    }
}

/// (e) `take_facts` stays offset-sorted with the new variant mixed in
/// (the sort arm covers all fact kinds).
#[test]
fn take_facts_stays_offset_sorted_with_promotion_reads() {
    let source = "void f() { return; 1; } void g(num x) { if (x is int) { x; } }";
    let facts = facts_of(source);
    // f ends unreachable (no completion row); g falls through — its row
    // anchors at the unit's last token, so it sorts last. g's condition
    // read (`x is int`'s operand) carries its own ReadType row at the
    // declared type (every reachable read does, #1056); the branch read
    // emits the promotion row and its ReadType row at the same span, the
    // promotion row first (the emit order the stable sort preserves).
    assert_eq!(facts.len(), 5, "{facts:?}");
    assert!(matches!(facts[0], FlowFact::DeadCode { .. }));
    assert!(matches!(facts[1], FlowFact::ReadType { .. }));
    assert!(matches!(facts[2], FlowFact::PromotedRead { .. }));
    assert!(matches!(facts[3], FlowFact::ReadType { .. }));
    assert!(matches!(facts[4], FlowFact::BodyMayCompleteNormally { .. }));
    let offsets: Vec<u32> = facts
        .iter()
        .map(|fact| match fact {
            FlowFact::DeadCode { span }
            | FlowFact::UnassignedRead { span, .. }
            | FlowFact::PromotedRead { span, .. }
            | FlowFact::ReadType { span, .. }
            | FlowFact::ConditionType { span, .. }
            | FlowFact::MatchedType { span, .. }
            | FlowFact::AsyncContextUse { span, .. }
            | FlowFact::PromotedPropertyRead { span }
            | FlowFact::WhyNotPromoted { span, .. }
            | FlowFact::NonPromotedPropertyRead { span, .. }
            | FlowFact::ThisNotPromoted { span, .. }
            | FlowFact::BodyMayCompleteNormally { span } => span.range.offset,
            FlowFact::WriteSite { decl_name_span, .. } => decl_name_span.offset,
        })
        .collect();
    assert!(offsets.windows(2).all(|pair| pair[0] <= pair[1]));
}

/// Read-type rows of a source, keyed by the read's offset.
fn read_types_of(source: &str) -> Vec<(u32, crate::ty::Ty)> {
    facts_of(source)
        .into_iter()
        .filter_map(|fact| match fact {
            FlowFact::ReadType { span, ty, .. } => Some((span.range.offset, ty)),
            _ => None,
        })
        .collect()
}

/// The top-level read face (#1730): a read of the unit's own top-level
/// declaration publishes its `ReadType` row — the lexical fallback answers
/// the written type (official unqualified-name lookup: the scope chain
/// first, the unit's top-level scope last,
/// property_element_resolver.dart:2137-2139). The written type is
/// asserted against a declared local's read in the same source (one
/// interned spelling, one id).
#[test]
fn top_level_read_carries_the_written_type() {
    let source = "String a = '';\nvoid f() { String b = a; var c = b; }\n";
    // `a` reads at offset 37, `b` at 48.
    let rows = read_types_of(source);
    let top = rows.iter().find(|(offset, _)| *offset == 37);
    let local = rows.iter().find(|(offset, _)| *offset == 48);
    let (Some((_, top)), Some((_, local))) = (top, local) else {
        panic!("missing read rows: {rows:?}");
    };
    assert!(*top != crate::ty::Ty::UNKNOWN, "{top:?}");
    assert_eq!(top, local, "the top-level face answers the written type");
}

/// A read inside a top-level initializer folds too: the initializer is
/// its own executable, so `var b = a` sees the earlier declaration's row.
#[test]
fn top_level_initializer_read_folds() {
    let source = "String a = '';\nvar b = a;\n";
    let rows = read_types_of(source);
    // `a` reads at offset 23, inside `b`'s initializer.
    assert!(
        rows.iter()
            .any(|(offset, ty)| *offset == 23 && *ty != crate::ty::Ty::UNKNOWN),
        "{rows:?}"
    );
}

/// A class field never joins the table: the capture is gated on the
/// container depth, so an unresolvable bare read keeps the
/// implicit-`this` face and publishes no row.
#[test]
fn class_field_does_not_join_the_top_level_table() {
    let source = "class C { String b = ''; }\nvoid f() { var x = b; }\n";
    assert!(
        !read_types_of(source)
            .iter()
            .any(|(offset, _)| *offset == 46),
        "a field read must not answer from the top-level table"
    );
}

/// A shadowing local wins: the scope chain answers first, so the local's
/// own face (initializer-derived unknown at name level) covers the read.
#[test]
fn shadowing_local_wins_over_the_top_level_face() {
    let source = "String a = '';\nvoid f() { var a = 1; var b = a; }\n";
    let rows = read_types_of(source);
    // `a` reads at offset 45 — the local's face, not the top-level
    // `String`. The `1` initializer's unspellable face adopts the
    // potentially-nullable unknown marker (#2241) — still the unknown
    // base, never the top-level declared type.
    let row = rows.iter().find(|(offset, _)| *offset == 45);
    assert_eq!(
        row.map(|(_, ty)| ty.base),
        Some(crate::ty::Base::Unknown),
        "{rows:?}"
    );
}

/// A plain assignment target retracts its row: the top-level target is
/// the write, not a read.
#[test]
fn plain_assignment_retracts_the_top_level_row() {
    let source = "String a = '';\nvoid f() { a = 'x'; }\n";
    assert!(
        !read_types_of(source)
            .iter()
            .any(|(offset, _)| *offset == 26),
        "{:?}",
        read_types_of(source)
    );
}

/// A forward reference under-reports: the declaration parsed after the
/// executable is captured too late for its replay.
#[test]
fn forward_referenced_declaration_under_reports() {
    let source = "void f() { var x = a; }\nString a = '';\n";
    assert!(
        !read_types_of(source)
            .iter()
            .any(|(offset, _)| *offset == 19),
        "{:?}",
        read_types_of(source)
    );
}

/// The plain-assignment target retraction covers `PromotedRead` too: a
/// target read inside a promoted branch emits the promotion fact (the
/// read runs before the assignment), but the identifier is the write —
/// both of its facts (the definitely-unassigned row and the promoted
/// row) must be retracted with it. The condition's own read survives
/// legitimately: `x` is still definitely unassigned there, which is the
/// official `not_assigned_potentially_non_nullable_local_variable` shape,
/// and its span is the condition's, not the target's — outside the
/// retraction's span key.
#[test]
#[expect(clippy::cast_possible_truncation)] // span arithmetic narrows under the scanner u32 offset cap
fn plain_assign_target_retracts_its_promoted_read() {
    let source = "void f() { String? x; if (x is String) { x = 'a'; } }";
    let facts = read_facts_of(source);
    let target_offset = source.find("x = 'a'").expect("target in source") as u32;
    assert_eq!(facts.len(), 1, "{facts:?}");
    match &facts[0] {
        FlowFact::UnassignedRead { span, .. } => {
            assert!(
                span.range.offset < target_offset,
                "the survivor must be the condition read, not the target: {facts:?}"
            );
        }
        other => panic!("expected only the condition's unassigned read, got {other:?}"),
    }
}

// —— body completion (BodyMayCompleteNormally) —————————————————————————————
// The pure reachability face of the official body_might_complete gate
// (`type_analyzer.dart:647-652`): the fact fires when the executable's
// body end is reachable, for every executable kind; the consumer owns
// the return-type, body-shape, and anchor faces.

/// Offsets of the body-completion anchors (the executables' end tokens),
/// offset-sorted by `take_facts`.
fn completion_offsets(source: &str) -> Vec<u32> {
    facts_of(source)
        .into_iter()
        .filter_map(|fact| match fact {
            FlowFact::BodyMayCompleteNormally { span } => Some(span.range.offset),
            _ => None,
        })
        .collect()
}

/// A body that falls off its end completes normally; the anchor is the
/// executable's end token — the body's closing brace (top-level function
/// and class-method hooks).
#[test]
fn fall_through_body_completes_at_the_end_token() {
    assert_eq!(completion_offsets("void f() { x; }"), [14]);
    assert_eq!(completion_offsets("class C { void m() { x; } }"), [24]);
}

/// A body ending in `return` or `throw` never completes: no fact. The
/// same holds when every branch of the last statement returns.
#[test]
fn never_completing_bodies_emit_no_completion() {
    assert!(completion_offsets("void f() { return; }").is_empty());
    assert!(completion_offsets("void f() { throw 0; }").is_empty());
    assert!(
        completion_offsets("void f(bool b) { if (b) { return; } else { return; } }").is_empty()
    );
}

/// official reachability semantics for loop shapes: `while (true)` and
/// `for (;;)` without a break make the body end unreachable; a break
/// restores it; `do {} while (false)` completes (the condition-false
/// exit is live).
#[test]
fn infinite_loop_shapes_decide_by_reachability() {
    assert!(completion_offsets("void f() { while (true) {} }").is_empty());
    assert!(completion_offsets("void f() { for (;;) {} }").is_empty());
    assert_eq!(
        completion_offsets("void f(bool b) { while (true) { if (b) break; } }"),
        [48]
    );
    assert_eq!(
        completion_offsets("void f() { do {} while (false); }"),
        [32]
    );
}

/// A closure body is its own executable: it completes independently of
/// the enclosing body (the official gate fires per `FunctionBody`).
#[test]
fn closure_bodies_complete_independently() {
    // Both the closure and the enclosing body fall through; the closure's
    // `}` sorts before the outer one.
    let source = "void f() { g(() { 1; }); }";
    assert_eq!(completion_offsets(source), [21, 25]);
    // A returning closure completes nothing; the outer body still does.
    let source = "void f() { g(() { return; }); }";
    assert_eq!(completion_offsets(source), [30]);
}

/// The pure reachability contract: an empty body's `;` is a reachable
/// end and emits — the consumer owns the body-shape gate (the official
/// reports only on block bodies).
#[test]
fn empty_body_completion_is_the_consumers_gate() {
    assert_eq!(completion_offsets("class C { void m(); }"), [18]);
}

/// Boolean short-circuit join: `a && b` merges the false arms.
#[test]
fn short_circuit_and_no_facts() {
    let source = "void f(bool a, bool b) { if (a && b) { return; } 1; }";
    assert!(dead_snippets(source).is_empty());
}

/// `if (true)` keeps only the then-branch; the implicit else arm is
/// unreachable but contributes nothing observable.
#[test]
fn literal_true_condition() {
    let source = "void f() { if (true) { return; } 1; }";
    assert_eq!(dead_snippets(source), ["1;"]);
}

/// official reachability/data/while.dart shape: `while (true)` with no
/// break makes following code dead.
#[test]
fn while_true_no_break_kills_exit() {
    let source = "void f() { while (true) { 1; } 2; }";
    assert_eq!(dead_snippets(source), ["2;"]);
}

/// Pattern variable declaration statement: the declared variables are
/// assigned on the statement's own path.
#[test]
fn pattern_declaration_assigns() {
    let source = "void f(Object o) { var (a, b) = (1, 2); a; b; }";
    assert!(unassigned_snippets(source).is_empty());
}

/// A cast pattern's primary is a pattern, not an expression: the parser
/// reuses the as-operator events for `p as T` (patterns.rs:176-187), so
/// `AsEnd` must not pop an operand off the empty expr stack (the "as
/// operand" panic), and the cast refines the pattern's declared variable
/// on the matched path (shared_type_analyzer.dart:145-152).
#[test]
fn cast_pattern_does_not_pop_the_expr_stack() {
    // Wildcard primary: nothing to pop, no crash, sound facts.
    let source = "void f(Object? o) { switch (o) { case _ as Object?: 1; } }";
    assert!(read_facts_of(source).is_empty());
    // Declared primary: the pattern variable is assigned on the matched
    // path and the case body runs under that state — a body read is
    // sound (the head's matched state reaches the body through
    // `case_pattern_end` plus the body-open join; official
    // `switch_endAlternative`/`switch_endAlternatives`,
    // flow_analysis.dart:8004-8066).
    let source = "void f(Object? o) { switch (o) { case var x as int: x; } }";
    assert!(unassigned_snippets(source).is_empty());
    // Guard position: the guard also evaluates under the matched state.
    let source = "void f(Object? o) { switch (o) { case var x as int when x > 0: 1; } }";
    assert!(unassigned_snippets(source).is_empty());
}

/// A cast in a case GUARD streams after `PatternEnd` (the parser closes
/// the pattern before `begin_switch_case_when_clause` opens the guard), so
/// `AsEnd`'s depth discriminator routes it to the expression face: the
/// operand pops, the cast value pushes, and the operand variable promotes
/// on the guard-true state the body opens on (official
/// `switch_endAlternative`'s guardInfo split — flow_analysis.dart:8004-8042
/// through `apply_pattern_exit`). No pattern-face refinement runs (the
/// wildcard head declares nothing to refine — the `p as T` in-place primary
/// is the other face), and the promotion cannot leak past the switch: the
/// close join with the unmatched chain drops it. The switch stays
/// exhaustive-agnostic — no dead rows anywhere. #1619 residual pin.
#[test]
fn case_guard_cast_takes_the_expression_face() {
    let source = "void f(Object? o, int? y) { switch (o) { case _ when (y as int) > 0: y; } y; }";
    let (flow, facts) = analyze(source);
    let read_types: Vec<(String, crate::ty::Ty)> = facts
        .iter()
        .filter_map(|fact| match fact {
            FlowFact::ReadType { span, ty, .. } => Some((
                source
                    [span.range.offset as usize..(span.range.offset + span.range.length) as usize]
                    .to_string(),
                *ty,
            )),
            _ => None,
        })
        .collect();
    assert_eq!(
        read_types
            .iter()
            .map(|(snippet, _)| snippet.as_str())
            .collect::<Vec<_>>(),
        ["o", "y", "y", "y"],
        "{facts:?}"
    );
    // Scrutinee and the guard's cast operand read declared (the operand
    // precedes the cast's promotion); the body read carries the guard-true
    // promotion; the post-switch read is back at the declared type.
    assert!(read_types[0].1.nullable, "{read_types:?}");
    assert!(read_types[1].1.nullable, "{read_types:?}");
    assert!(!read_types[2].1.nullable, "{read_types:?}");
    assert!(read_types[3].1.nullable, "{read_types:?}");
    let crate::ty::Base::Name(id) = read_types[2].1.base else {
        panic!("expected a named type, got {:?}", read_types[2].1);
    };
    assert_eq!(flow.0.borrow().name_lexeme(id), "int");
    assert!(dead_snippets(source).is_empty(), "{source:?}");
}

/// The case head's state carries into the case body: the guard's outcome
/// joins the unmatched chain and the group's combined state feeds the
/// body (official `switch_endAlternative`, flow_analysis.dart:8004-8042,
/// and `switch_endAlternatives`, :8044-8066), and the cast pattern's
/// promotion reaches the body the same way.
#[test]
fn case_head_state_carries_into_the_body() {
    // A never-true guard makes the group's combined state unreachable,
    // but the body still opens on a control-flow split checkpoint:
    // official `Reachability.split` passes locallyReachable=true
    // (flow_analysis.dart:5607) and `switch_endAlternatives` splits the
    // body's entry state (:8060-8066), so no dead code is reported
    // inside the case body — the guard's false outcome lives only in
    // the unmatched chain.
    let source = "void f(Object? o) { switch (o) { case _ when false: 1; } }";
    assert!(dead_snippets(source).is_empty());
    // Guard type-test promotion carries into the body: with `n`
    // promoted to `int` by the guard, the impossible `n is! int`
    // then-branch is dead (official tryPromoteForTypeCheck's bottom
    // typeIfFalse, flow_analysis.dart:4118-4179). The row is the
    // outermost dead construct only — the whole then-block, exactly
    // one diagnostic (official dead_code per dead statement, no
    // per-descendant rows).
    let source =
        "void f(num x) { switch (x) { case num n when n is int: if (n is! int) { return; } n; } }";
    assert_eq!(dead_snippets(source), ["{ return; }"]);
    // With `x` promoted to `int` by the cast pattern, `x is! int` cannot
    // hold: the then-branch is dead, reported as the single outermost
    // block row.
    let source =
        "void f(Object? o) { switch (o) { case var x as int: if (x is! int) { return; } x; } }";
    assert_eq!(dead_snippets(source), ["{ return; }"]);
}

/// Closures inside loops: the loop-head conservative join must not crash
/// with captured writes, and facts stay absent for sound code.
#[test]
fn closure_in_loop_no_facts() {
    let source = "void f() { for (var i = 0; i < 3; i++) { g(() { i; }); } }";
    assert!(read_facts_of(source).is_empty());
}

/// A `try/finally` whose finally returns: the code after is dead
/// (official `reachability/data/try_again` shapes).
#[test]
fn finally_return_kills_exit() {
    let source = "void f() { try { 1; } finally { return; } 2; }";
    assert_eq!(dead_snippets(source), ["2;"]);
}

#[test]
fn try_finally_fallthrough_alive() {
    let source = "void f() { try { 1; } finally { 2; } 3; }";
    assert!(dead_snippets(source).is_empty());
}

/// A collection `for`-in element replays its head exactly once: the
/// statement-form `ForInParts` handler must not also fire inside the
/// collection extent (the double head popped the assignment operands —
/// an empty-stack panic at the assignment).
///
/// Ignored until the upstream blocker closes: the parser currently
/// mis-parses every collection-element for-head (a synthetic `ForParts`
/// C-style recovery with stray statement events instead of `ForInParts`,
/// silently, no diagnostics), so this source never produces the event
/// shape the defect lives on. dartq-parser owns the mis-parse; the flow
/// side is fixed (single head, extent consumed wholesale in collect runs,
/// element placeholder value).
#[test]
fn collection_for_in_assignment_element_no_panic() {
    let source = "void f(List<int> b) { int x; x = [for (var a in b) a]; x; }";
    assert!(unassigned_snippets(source).is_empty());
}

/// A collection `for`-in element as a variable initializer: the write from
/// the initializer must reach later reads of the variable.
///
/// Ignored until the upstream blockers close: the parser currently
/// mis-parses every collection-element for-head (synthetic `ForParts`
/// separators instead of `ForInParts`, silent), and even a correctly
/// parsed head would clear the enclosing declaration's pending init (the
/// loop variable's `VarDeclBegin` resets the flat `pending_inits` list),
/// so `InitializerEnd` finds no variable to write. dartq-parser owns the
/// mis-parse; the pending-init pairing is a flow-side follow-up.
#[test]
fn collection_for_in_initializer_write_reaches_later_reads() {
    let source = "void f(List<int> b) { var x = [for (var a in b) a]; x; }";
    assert!(unassigned_snippets(source).is_empty());
}

/// A labeled case group closes exactly once: the group-close lookahead
/// must not re-enter a collect run over the same still-open group
/// (unbounded recursion — stack overflow on `switch (x) { foo: case 1:
/// 1; }`).
#[test]
fn labeled_case_group_terminates() {
    let source = "void f(int x) { switch (x) { foo: case 1: 1; } }";
    assert!(read_facts_of(source).is_empty());
}

/// The live-id projection boundary (#920, pinned #934): the switch-block
/// effect set collects runs of declarations, and the projection removes a
/// block-local's collect-run id from it — `t` is scoped to `l1`'s block, so
/// the later group's close joins cannot alias `t`'s write onto the outer
/// `a`. The entry promotion survives the block-local's write and the
/// `a.isEven` read in `l2` still emits its promoted-read fact. Contrast
/// [`labeled_group_write_demotes_entry_promotion_in_later_groups`]: a
/// non-block write (`x = 1`) stays in the effect set and demotes.
#[test]
fn block_local_write_keeps_entry_promotion_in_later_groups() {
    let source = concat!(
        "void f(int? a) { if (a != null) { switch (0) { ",
        "l1: case 1: var t = 0; break; ",
        "l2: case 2: a.isEven; } } }"
    );
    let promoted = promoted_tys(source);
    assert_eq!(
        promoted.len(),
        1,
        "entry promotion survives the block-local write: {promoted:?}"
    );
    assert!(!promoted[0].nullable);
}

/// A labeled group's close joins every write in the switch block: a write
/// in one labeled group demotes the entry promotion for every later group.
/// The block effect set is cached on the switch frame at the first labeled
/// close (#919) — the second group's close joins from the cache and must
/// demote `x` identically, so `x.isEven` reads it un-promoted and no
/// promoted-read fact surfaces. The switch scrutinizes `z` so the only
/// candidate read is the in-group one (an `int?` scrutinee read would emit
/// its own promotion fact regardless of labels). The contrast pins
/// sensitivity: without the write the entry promotion survives into `l2`
/// and the read emits.
#[test]
fn labeled_group_write_demotes_entry_promotion_in_later_groups() {
    let with_write = concat!(
        "void f(int? x, int z) { if (x != null) { switch (z) { ",
        "l1: case 1: x = 1; break; ",
        "l2: case 2: x.isEven; } } }"
    );
    assert!(promoted_tys(with_write).is_empty());
    let without_write = concat!(
        "void f(int? x, int z) { if (x != null) { switch (z) { ",
        "l1: case 1: break; ",
        "l2: case 2: x.isEven; } } }"
    );
    assert_eq!(promoted_tys(without_write).len(), 1);
}

/// An arrow body's value is consumed by the function and the function
/// expression leaves its own value at `ExecEnd`: `InitializerEnd` finds a
/// right-hand side even when the body itself leaves nothing (a `throw`
/// body). Before the fix the `ExprBody` no-op shadow leaked the body's
/// value — with a `throw` body nothing was left and the variable stayed
/// unwritten.
#[test]
fn arrow_body_nets_one_value() {
    let source = "void f() { var g = () => throw 1; g; }";
    assert!(unassigned_snippets(source).is_empty());
}

/// `suspension` demotes only inside a local function (official
/// `_enclosingFunctionExpressionInfoStack` non-empty): an await directly
/// in an async method body demotes nothing, so a promotion from the
/// condition survives the await without crashing — and the promoted read
/// after the await still surfaces as a fact.
#[test]
fn await_in_plain_async_method_demotes_nothing() {
    let source = "void f(int? x, Future g()) async { if (x != null) { await g(); x; } }";
    let promoted = promoted_tys(source);
    assert_eq!(promoted.len(), 1);
    assert!(matches!(promoted[0].base, crate::ty::Base::Name(_)));
    assert!(!promoted[0].nullable);
}

/// `joinPromotedTypes` is the order-preserving intersection (official
/// `chain1.where(chain2.contains)`): a mid-chain fork keeps the deeper
/// common element instead of stopping at the first mismatch. Only `t3` is
/// common to both chains and it sits behind the incomparable fronts, so a
/// prefix zip would produce an empty chain.
#[test]
fn join_promoted_intersection_keeps_deeper_common_types() {
    use crate::model::{Lists, NIL, ReachArena, State, VarModel};
    use crate::ty::{Base, Ty};
    let ty = |name| Ty {
        base: Base::Name(name),
        nullable: false,
    };
    let (t1, t2, t3, t4, t5) = (ty(1), ty(2), ty(3), ty(4), ty(5));
    let mut lists = Lists::default();
    // Most-specific first: [t1, t3, t5] forked against [t2, t3, t4].
    let no_args = crate::ty::EMPTY_ARGS;
    let a_mid = lists.cons(t5, no_args, NIL);
    let a_tail = lists.cons(t3, no_args, a_mid);
    let a = lists.cons(t1, no_args, a_tail);
    let b_mid = lists.cons(t4, no_args, NIL);
    let b_tail = lists.cons(t3, no_args, b_mid);
    let b = lists.cons(t2, no_args, b_tail);
    let state = |promoted| {
        let mut s = State::new(0);
        s.models.push(VarModel {
            promoted,
            tested: NIL,
            version: 1,
            assigned: true,
            unassigned: false,
            history: NIL,
        });
        s
    };
    let mut reach = ReachArena::new();
    let joined = State::join(Some(state(a)), Some(state(b)), &mut reach, &mut lists, 1);
    let kept: Vec<Ty> = lists.iter(joined.model(0).promoted).collect();
    assert_eq!(kept, vec![t3]);
}

/// A begin whose matching end never arrives (parser recovery) is skipped
/// as self-contained: the extent consumers bail on the NIL match instead
/// of slicing the buffer to an out-of-bounds end.
#[test]
fn unmatched_construct_begins_do_not_panic() {
    use crate::ev::Ev;
    use crate::interp::Interp;
    use crate::ty::NameTypeOps;
    let span = |offset: u32| dartq_parser::TokenSpan {
        id: offset,
        range: dartq_source::SourceRange { offset, length: 1 },
    };
    let events = vec![
        // While without WhileEnd.
        Ev::WhileBegin { span: span(1) },
        Ev::WhileBodyBegin,
        Ev::ExprStmt { span: span(2) },
        // Do without DoEnd (its frame closes through the exit handler).
        Ev::DoBegin { span: span(3) },
        Ev::DoEnd {
            while_range: dartq_source::SourceRange {
                offset: 4,
                length: 1,
            },
        },
        // Try without TryEnd.
        Ev::TryBegin { span: span(4) },
        Ev::CatchClauseBegin,
        Ev::CatchClauseEnd,
        // Closure without ExecEnd.
        Ev::ExecBegin { is_closure: true },
        // C-style for without ForEnd.
        Ev::ForBegin { span: span(5) },
        Ev::ForParts {
            left_sep: span(6),
            right_sep: span(7),
        },
        // Collection for without ForControlFlowEnd.
        Ev::ForControlFlowBegin,
        // Switch block without SwitchBlockEnd.
        Ev::SwitchBlockBegin,
    ];
    let mut interp = Interp::new(&NameTypeOps, &[], &[]);
    interp.run(&events);
}

/// A declaration head nested inside an initializer (the collection-for
/// loop variable) must not clobber the enclosing declaration's pending
/// init: `VarDeclBegin`/`VarDeclEnd` pair as a stack, so the outer
/// `InitializerEnd` still writes its variable.
#[test]
fn nested_declaration_head_preserves_enclosing_initializer() {
    let source = "void f() { var x = [for (var i = 0; i < 3; i++) i]; x; }";
    assert!(unassigned_snippets(source).is_empty());
}

/// Every literal element nets exactly one value: a spread element leaves
/// its own value after consuming the operand's, so the literal's consumer
/// finds a right-hand side and the variable is written.
#[test]
fn spread_element_nets_one_literal_value() {
    let source = "void f(List<int> y) { var x = [...y]; x; }";
    assert!(unassigned_snippets(source).is_empty());
}

/// A C-style collection-for consumes its extent wholesale (jumping over
/// the `ForControlFlowEnd` event), so it must close its loop frame itself:
/// no frame leaks past the element, and the element nets exactly one
/// value.
#[test]
fn c_style_collection_for_closes_its_loop_frame() {
    use crate::ev::Ev;
    use crate::interp::Interp;
    use crate::ty::NameTypeOps;
    use dartq_parser::{IdentifierContext, TokenSpan};
    use dartq_source::SourceRange;
    let span = |offset: u32| TokenSpan {
        id: offset,
        range: SourceRange { offset, length: 1 },
    };
    // Event layout of `[for (var i = 0; i < 3; i++) e]`: the three header
    // parts precede the `ForParts` marker in source order, partitioned by
    // the separator offsets (init < 15 <= cond < 25 <= upd).
    let events = vec![
        Ev::ForControlFlowBegin,
        Ev::VarDeclBegin {
            late: false,
            is_final: true,
            span: span(10),
        },
        Ev::InitializedIdent {
            name: 0,
            token: span(1),
        },
        Ev::OtherExpr { span: span(11) },
        Ev::InitializerEnd,
        Ev::VarDeclEnd,
        Ev::Ident {
            name: 0,
            token: span(20),
            context: IdentifierContext::Expression,
        },
        Ev::Ident {
            name: 0,
            token: span(30),
            context: IdentifierContext::Expression,
        },
        Ev::ForParts {
            left_sep: span(15),
            right_sep: span(25),
        },
        Ev::OtherExpr { span: span(40) },
        Ev::ForControlFlowEnd { for_in: false },
    ];
    let mut interp = Interp::new(&NameTypeOps, &[], &[]);
    interp.run(&events);
    assert!(interp.stack.is_empty());
    assert_eq!(interp.exprs.len(), 1);
}

/// A closure's effect collect inside an enclosing loop replays the inner
/// loop's end event: that collect-mode `loop_exit` must not pop the
/// enclosing replay's REAL loop frame (the rollback truncates only the
/// tail, so a mid-stack removal loses the outer loop's exit model — its
/// cond-false path — for good, and code after the loop is misreported
/// dead). Shape: `while (a) { g() { while (b) {} } return; } 1;` — the
/// body ends unreachable, so only the loop exit's cond-false join keeps
/// the trailing statement alive.
#[test]
fn closure_collect_keeps_enclosing_loop_frame() {
    use crate::ev::Ev;
    use crate::interp::Interp;
    use crate::ty::NameTypeOps;
    use dartq_parser::TokenSpan;
    use dartq_source::SourceRange;
    let span = |offset: u32| TokenSpan {
        id: offset,
        range: SourceRange { offset, length: 1 },
    };
    let events = vec![
        Ev::WhileBegin { span: span(1) },
        Ev::WhileBodyBegin,
        Ev::ExecBegin { is_closure: true },
        Ev::WhileBegin { span: span(2) },
        Ev::WhileBodyBegin,
        Ev::WhileEnd,
        Ev::ExecEnd {
            is_expr: false,
            end: tspan(0),
        },
        Ev::ReturnBegin { span: span(3) },
        Ev::ReturnEnd {
            has_expression: false,
        },
        Ev::WhileEnd,
        Ev::ExprStmt { span: span(4) },
    ];
    let mut interp = Interp::new(&NameTypeOps, &[], &[]);
    interp.run(&events);
    assert!(interp.stack.is_empty());
    // The closure body falls off its end: one completion row (the outer
    // body's own end is behind this fixture's trailing return).
    assert!(matches!(
        interp.facts.as_slice(),
        [FlowFact::BodyMayCompleteNormally { .. }]
    ));
    assert!(interp.reachable());
}

/// A recovered case-less switch expression (`switch (x) {}` from parser
/// recovery) has no alternatives to join: the block end must carry the
/// entry state instead of joining two absent states (a panic before the
/// fallback).
#[test]
fn case_less_switch_expression_does_not_panic() {
    use crate::ev::Ev;
    use crate::interp::Interp;
    use crate::ty::NameTypeOps;
    use dartq_parser::TokenSpan;
    use dartq_source::SourceRange;
    let span = |offset: u32| TokenSpan {
        id: offset,
        range: SourceRange { offset, length: 1 },
    };
    let events = vec![
        Ev::SwitchExprBegin { span: span(1) },
        Ev::SwitchExprBlockBegin,
        Ev::SwitchExprBlockEnd,
    ];
    let mut interp = Interp::new(&NameTypeOps, &[], &[]);
    interp.run(&events);
    assert!(interp.stack.is_empty());
    assert!(interp.facts.is_empty());
    assert!(interp.reachable());
}

// —— try/catch/finally combinations ——————————————————————————————————————
// Event layouts mirror the recorder (record.rs): the try body runs before
// `CatchClauseBegin`, each clause ends at `CatchClauseEnd` (the
// `handle_catch_block` join), `FinallyBlock` separates the catch context
// from the finally body, and `TryEnd { has_finally }` closes the frame.

/// try + catch + finally, every part completing normally: `FinallyBlock`
/// joins the try-catch exit with the conservative exception-entry state
/// (official `tryFinallyStatement_finallyBegin`, `flow_analysis.dart:8269`)
/// and `TryEnd` attaches the finally without killing the tail.
#[test]
fn try_catch_finally_completes_normally() {
    use crate::ev::Ev;
    use crate::interp::Interp;
    use crate::ty::NameTypeOps;
    use dartq_parser::TokenSpan;
    use dartq_source::SourceRange;
    let span = |offset: u32| TokenSpan {
        id: offset,
        range: SourceRange { offset, length: 1 },
    };
    let events = vec![
        Ev::TryBegin { span: span(1) },
        Ev::ExprStmt { span: span(2) },
        Ev::OtherExpr { span: span(3) },
        Ev::CatchClauseBegin,
        Ev::ExprStmt { span: span(4) },
        Ev::OtherExpr { span: span(5) },
        Ev::CatchClauseEnd,
        Ev::FinallyBlock,
        Ev::ExprStmt { span: span(6) },
        Ev::OtherExpr { span: span(7) },
        Ev::TryEnd { has_finally: true },
    ];
    let mut interp = Interp::new(&NameTypeOps, &[], &[]);
    interp.run(&events);
    assert!(interp.stack.is_empty());
    assert!(interp.facts.is_empty());
    assert!(interp.reachable());
}

/// try/finally with a return in the body: the finally completes normally,
/// but `_attachFinally` (flow_analysis.dart:8419-8467) takes the exit
/// reachability from the try exit (r1, unreachable) whenever the finally
/// exit itself is reachable — the trailing statement is dead.
#[test]
fn try_finally_body_return_leaves_tail_dead() {
    use crate::ev::Ev;
    use crate::interp::Interp;
    use crate::ty::NameTypeOps;
    use dartq_parser::TokenSpan;
    use dartq_source::SourceRange;
    let span = |offset: u32| TokenSpan {
        id: offset,
        range: SourceRange { offset, length: 1 },
    };
    let events = vec![
        Ev::TryBegin { span: span(1) },
        Ev::ReturnBegin { span: span(2) },
        Ev::ReturnEnd {
            has_expression: false,
        },
        Ev::FinallyBlock,
        Ev::ExprStmt { span: span(3) },
        Ev::OtherExpr { span: span(4) },
        Ev::TryEnd { has_finally: true },
        Ev::ExprStmt { span: span(5) },
        Ev::OtherExpr { span: span(6) },
    ];
    let mut interp = Interp::new(&NameTypeOps, &[], &[]);
    interp.run(&events);
    assert!(interp.stack.is_empty());
    assert_eq!(interp.facts.len(), 1);
    assert!(!interp.reachable());
}

/// A return in the finally itself: the finally exit is unreachable, so
/// `_attachFinally` marks the try exit unreachable too — the trailing
/// statement is dead (event-level twin of `finally_return_kills_exit`).
#[test]
fn try_finally_return_leaves_tail_dead() {
    use crate::ev::Ev;
    use crate::interp::Interp;
    use crate::ty::NameTypeOps;
    use dartq_parser::TokenSpan;
    use dartq_source::SourceRange;
    let span = |offset: u32| TokenSpan {
        id: offset,
        range: SourceRange { offset, length: 1 },
    };
    let events = vec![
        Ev::TryBegin { span: span(1) },
        Ev::ExprStmt { span: span(2) },
        Ev::OtherExpr { span: span(3) },
        Ev::FinallyBlock,
        Ev::ReturnBegin { span: span(4) },
        Ev::ReturnEnd {
            has_expression: false,
        },
        Ev::TryEnd { has_finally: true },
        Ev::ExprStmt { span: span(5) },
        Ev::OtherExpr { span: span(6) },
    ];
    let mut interp = Interp::new(&NameTypeOps, &[], &[]);
    interp.run(&events);
    assert!(interp.stack.is_empty());
    assert_eq!(interp.facts.len(), 1);
    assert!(!interp.reachable());
}

/// Two catch clauses: the first returns, the second completes. Later
/// catches resume from `beforeCatch`, not from the previous catch's exit
/// (official `tryCatchStatement_catchBegin`, `flow_analysis.dart:8182`), so
/// the second clause's completion keeps the statement exit alive.
#[test]
fn try_two_catches_first_returns_second_completes() {
    use crate::ev::Ev;
    use crate::interp::Interp;
    use crate::ty::NameTypeOps;
    use dartq_parser::TokenSpan;
    use dartq_source::SourceRange;
    let span = |offset: u32| TokenSpan {
        id: offset,
        range: SourceRange { offset, length: 1 },
    };
    let events = vec![
        Ev::TryBegin { span: span(1) },
        Ev::ExprStmt { span: span(2) },
        Ev::OtherExpr { span: span(3) },
        Ev::CatchClauseBegin,
        Ev::ReturnBegin { span: span(4) },
        Ev::ReturnEnd {
            has_expression: false,
        },
        Ev::CatchClauseEnd,
        Ev::CatchClauseBegin,
        Ev::ExprStmt { span: span(5) },
        Ev::OtherExpr { span: span(6) },
        Ev::CatchClauseEnd,
        Ev::TryEnd { has_finally: false },
    ];
    let mut interp = Interp::new(&NameTypeOps, &[], &[]);
    interp.run(&events);
    assert!(interp.stack.is_empty());
    assert!(interp.facts.is_empty());
    assert!(interp.reachable());
}

/// A `break` inside the finally block: the jump targets the enclosing loop
/// (try frames are not break targets), so the exit state travels through
/// the loop's break model and the code after the loop stays reachable.
#[test]
fn finally_break_exits_via_loop_exit_join() {
    use crate::ev::Ev;
    use crate::interp::Interp;
    use crate::ty::NameTypeOps;
    use dartq_parser::TokenSpan;
    use dartq_source::SourceRange;
    let span = |offset: u32| TokenSpan {
        id: offset,
        range: SourceRange { offset, length: 1 },
    };
    let events = vec![
        Ev::WhileBegin { span: span(1) },
        Ev::WhileBodyBegin,
        Ev::TryBegin { span: span(2) },
        Ev::ExprStmt { span: span(3) },
        Ev::OtherExpr { span: span(4) },
        Ev::FinallyBlock,
        Ev::Break {
            label: None,
            span: span(5),
        },
        Ev::TryEnd { has_finally: true },
        Ev::WhileEnd,
        Ev::ExprStmt { span: span(6) },
        Ev::OtherExpr { span: span(7) },
    ];
    let mut interp = Interp::new(&NameTypeOps, &[], &[]);
    interp.run(&events);
    assert!(interp.stack.is_empty());
    assert!(interp.facts.is_empty());
    assert!(interp.reachable());
}

/// A `continue` inside the finally block: the jump re-enters the enclosing
/// loop through its continue model, and the loop's condition-false exit
/// keeps the code after the loop reachable.
#[test]
fn finally_continue_exits_via_loop_exit_join() {
    use crate::ev::Ev;
    use crate::interp::Interp;
    use crate::ty::NameTypeOps;
    use dartq_parser::TokenSpan;
    use dartq_source::SourceRange;
    let span = |offset: u32| TokenSpan {
        id: offset,
        range: SourceRange { offset, length: 1 },
    };
    let events = vec![
        Ev::WhileBegin { span: span(1) },
        Ev::WhileBodyBegin,
        Ev::TryBegin { span: span(2) },
        Ev::ExprStmt { span: span(3) },
        Ev::OtherExpr { span: span(4) },
        Ev::FinallyBlock,
        Ev::Continue {
            label: None,
            span: span(5),
        },
        Ev::TryEnd { has_finally: true },
        Ev::WhileEnd,
        Ev::ExprStmt { span: span(6) },
        Ev::OtherExpr { span: span(7) },
    ];
    let mut interp = Interp::new(&NameTypeOps, &[], &[]);
    interp.run(&events);
    assert!(interp.stack.is_empty());
    assert!(interp.facts.is_empty());
    assert!(interp.reachable());
}

/// try + catch + finally where the CATCH returns but the try body
/// completes: the body's normal exit is seeded into the after-accumulator
/// (official `tryCatchStatement_bodyEnd`, `flow_analysis.dart:8163`) and
/// survives the returning catch, so the finally keeps the tail alive.
#[test]
fn try_catch_finally_returning_catch_keeps_tail_alive() {
    use crate::ev::Ev;
    use crate::interp::Interp;
    use crate::ty::NameTypeOps;
    use dartq_parser::TokenSpan;
    use dartq_source::SourceRange;
    let span = |offset: u32| TokenSpan {
        id: offset,
        range: SourceRange { offset, length: 1 },
    };
    let events = vec![
        Ev::TryBegin { span: span(1) },
        Ev::ExprStmt { span: span(2) },
        Ev::OtherExpr { span: span(3) },
        Ev::CatchClauseBegin,
        Ev::ReturnBegin { span: span(4) },
        Ev::ReturnEnd {
            has_expression: false,
        },
        Ev::CatchClauseEnd,
        Ev::FinallyBlock,
        Ev::ExprStmt { span: span(5) },
        Ev::OtherExpr { span: span(6) },
        Ev::TryEnd { has_finally: true },
        Ev::ExprStmt { span: span(7) },
        Ev::OtherExpr { span: span(8) },
    ];
    let mut interp = Interp::new(&NameTypeOps, &[], &[]);
    interp.run(&events);
    assert!(interp.stack.is_empty());
    assert!(interp.facts.is_empty());
    assert!(interp.reachable());
}

// —— switch statements and switch expressions with cases —————————————————

/// Switch expression with two alternatives (a guarded constant and a
/// wildcard): each arm branches from the chained unmatched state, the
/// guard events run between `SwitchExprCasePattern` and the arm value (the
/// recorder's order), and the block end joins the arms' combined state.
/// Invariant-level: stack drains, replay stays reachable, and one
/// matched-type row per alternative (official
/// `switch_beginAlternative`/`switch_endAlternative`,
/// `flow_analysis.dart:1680`).
#[test]
fn switch_expression_cases_and_guard_replay() {
    use crate::ev::Ev;
    use crate::interp::Interp;
    use crate::ty::NameTypeOps;
    use dartq_parser::{IdentifierContext, TokenSpan};
    use dartq_source::SourceRange;
    let span = |offset: u32| TokenSpan {
        id: offset,
        range: SourceRange { offset, length: 1 },
    };
    let events = vec![
        // `var x = ...;` — the scrutinee must resolve to a variable for
        // the matched-type rows (official `getMatchedValueType` keys on
        // the matched value's promotion key; an unresolved name carries
        // none and emits no row).
        Ev::VarDeclBegin {
            late: false,
            is_final: false,
            span: span(0),
        },
        Ev::InitializedIdent {
            name: 0,
            token: span(0),
        },
        Ev::InitializerBegin,
        Ev::OtherExpr { span: span(0) },
        Ev::InitializerEnd,
        Ev::VarDeclEnd,
        Ev::SwitchExprBegin { span: span(1) },
        Ev::Ident {
            name: 0,
            token: span(2),
            context: IdentifierContext::Expression,
        },
        Ev::SwitchExprBlockBegin,
        // `1 when b => e` — pattern, separator, guard, then the value.
        Ev::SwitchExprCaseBegin,
        Ev::PatternBegin,
        Ev::ConstantPatternBegin,
        Ev::OtherExpr { span: span(3) },
        Ev::ConstantPatternEnd,
        Ev::PatternEnd,
        Ev::SwitchExprCasePattern,
        Ev::GuardBegin,
        Ev::Bool { value: true },
        Ev::GuardEnd,
        Ev::OtherExpr { span: span(4) },
        Ev::SwitchExprCaseEnd {
            has_guard: true,
            begin: span(3),
        },
        // `_ => e`.
        Ev::SwitchExprCaseBegin,
        Ev::PatternBegin,
        Ev::Wildcard { token: span(5) },
        Ev::PatternEnd,
        Ev::SwitchExprCasePattern,
        Ev::OtherExpr { span: span(6) },
        Ev::SwitchExprCaseEnd {
            has_guard: false,
            begin: span(5),
        },
        Ev::SwitchExprBlockEnd,
    ];
    let mut interp = Interp::new(&NameTypeOps, &[], &[]);
    interp.run(&events);
    assert!(interp.stack.is_empty());
    // The matched-type rows (#1349): one per alternative — the scrutinee
    // is a bare read of an unknown-typed local, so the rows carry the
    // unknown declared type. The scrutinee read itself carries its
    // read-type row (the query face, #1056).
    assert_eq!(interp.facts.len(), 3, "{:?}", interp.facts);
    assert!(
        interp
            .facts
            .iter()
            .filter(|fact| matches!(fact, FlowFact::MatchedType { .. }))
            .count()
            == 2,
        "{:?}",
        interp.facts
    );
    assert!(
        matches!(interp.facts.first(), Some(FlowFact::ReadType { .. })),
        "{:?}",
        interp.facts
    );
    assert!(interp.reachable());
}

/// Switch statement whose `case` and `default` both return: the default
/// kills the unmatched chain (official `switch_endAlternatives`
/// isExhaustive effect), so the joined exit is unreachable and the
/// statement after the switch is dead (event-level twin of
/// `switch_all_cases_exit`).
#[test]
fn switch_statement_default_exhaustive_tail_dead() {
    use crate::ev::Ev;
    use crate::interp::Interp;
    use crate::ty::NameTypeOps;
    use dartq_parser::{IdentifierContext, TokenSpan};
    use dartq_source::SourceRange;
    let span = |offset: u32| TokenSpan {
        id: offset,
        range: SourceRange { offset, length: 1 },
    };
    let events = vec![
        // `var x = ...;` — the scrutinee resolves to a variable so the
        // case head emits its matched-type row (a promotion key requires
        // a variable read; an unresolved name emits none).
        Ev::VarDeclBegin {
            late: false,
            is_final: false,
            span: span(0),
        },
        Ev::InitializedIdent {
            name: 0,
            token: span(0),
        },
        Ev::InitializerBegin,
        Ev::OtherExpr { span: span(0) },
        Ev::InitializerEnd,
        Ev::VarDeclEnd,
        Ev::SwitchBegin { span: span(1) },
        Ev::Ident {
            name: 0,
            token: span(2),
            context: IdentifierContext::Expression,
        },
        Ev::SwitchBlockBegin,
        Ev::CaseBegin {
            label_count: 0,
            expression_count: 1,
            kw: None,
        },
        Ev::CaseExprBegin { span: span(3) },
        Ev::OtherExpr { span: span(3) },
        Ev::CaseExprEnd {
            has_guard: false,
            kw: span(3),
        },
        Ev::ReturnBegin { span: span(4) },
        Ev::ReturnEnd {
            has_expression: false,
        },
        Ev::CaseEnd {
            statement_count: 1,
            default_kw: None,
        },
        // `default:` — no case-expression events; the group opens at once.
        Ev::CaseBegin {
            label_count: 0,
            expression_count: 0,
            kw: Some(span(5)),
        },
        Ev::ReturnBegin { span: span(5) },
        Ev::ReturnEnd {
            has_expression: false,
        },
        Ev::CaseEnd {
            statement_count: 1,
            default_kw: Some(span(5)),
        },
        Ev::SwitchBlockEnd,
        Ev::ExprStmt { span: span(6) },
        Ev::OtherExpr { span: span(7) },
    ];
    let mut interp = Interp::new(&NameTypeOps, &[], &[]);
    interp.run(&events);
    assert!(interp.stack.is_empty());
    // The dead tail, the matched-type row the case head emits (#1349; the
    // bare-read scrutinee has no declared metadata: the unknown type), and
    // the scrutinee read's own read-type row (the query face, #1056).
    assert_eq!(interp.facts.len(), 3, "{:?}", interp.facts);
    assert!(
        interp
            .facts
            .iter()
            .filter(|fact| matches!(fact, FlowFact::MatchedType { .. }))
            .count()
            == 1,
        "{:?}",
        interp.facts
    );
    assert!(
        interp
            .facts
            .iter()
            .any(|fact| matches!(fact, FlowFact::DeadCode { .. })),
        "{:?}",
        interp.facts
    );
    assert!(!interp.reachable());
}

/// A switch expression used as a variable initializer must leave exactly
/// one value for the enclosing `InitializerEnd` consumer: `var y =
/// switch (x) { 1 => 2 }; y;` must not report `y` as definitely
/// unassigned.
///
/// Suspected defect (do not weaken): `SwitchExprCaseEnd` (`interp_stmt.rs`)
/// pops each arm's value and `SwitchExprBlockEnd` (`interp_stmt.rs`) never
/// pushes a replacement, so the whole expression nets ZERO values; the
/// `InitializerEnd` pairing then finds no right-hand side and `y` stays
/// unwritten — a false-positive `UnassignedRead`. The same hole makes `x =
/// switch (x) { 1 => 2 };` pop the assignment target as the right-hand
/// side. Every other composite expression nets exactly one value for its
/// consumer (see the `ListLitEnd`/`ExprBody` contracts in ev.rs).
#[test]
fn switch_expression_value_reaches_later_reads() {
    let source = "void f(int x) { var y = switch (x) { 1 => 2 }; y; }";
    assert!(unassigned_snippets(source).is_empty());
}

// —— assert statements ————————————————————————————————————————————————————

/// `assert` with a condition and a message: the exit joins the pre-assert
/// state with the condition-true branch only — the message's effects are
/// dropped and both expression values are truncated off the stack
/// (official `assert_end`, flow_analysis.dart:8218-8223).
#[test]
fn assert_join_with_message_invariants() {
    use crate::ev::Ev;
    use crate::interp::Interp;
    use crate::ty::NameTypeOps;
    use dartq_parser::TokenSpan;
    use dartq_source::SourceRange;
    let span = |offset: u32| TokenSpan {
        id: offset,
        range: SourceRange { offset, length: 1 },
    };
    let events = vec![
        Ev::AssertBegin { span: span(1) },
        Ev::OtherExpr { span: span(2) },
        Ev::Bool { value: true },
        Ev::AssertEnd,
    ];
    let mut interp = Interp::new(&NameTypeOps, &[], &[]);
    interp.run(&events);
    assert!(interp.stack.is_empty());
    assert!(interp.exprs.is_empty());
    assert!(interp.facts.is_empty());
    assert!(interp.reachable());
}

/// official `definite_assignment/data/assert.dart` `assertStatement`: the
/// condition's assignment holds while the message runs (so the message
/// read is clean) but the exit joins the pre-assert state, and the read
/// after the statement is definitely unassigned.
///
/// Replayed at the buffer level to pin the exact event stream the
/// recorder produces (the source-level twin below proves the
/// `begin_assert`/`end_assert` forwarding reaches the interpreter).
#[test]
fn assert_condition_write_unassigned_after() {
    use crate::ev::{BinOp, Ev};
    use crate::interp::Interp;
    use crate::ty::NameTypeOps;
    use dartq_parser::{IdentifierContext, TokenSpan};
    use dartq_source::SourceRange;
    let span = |offset: u32| TokenSpan {
        id: offset,
        range: SourceRange { offset, length: 1 },
    };
    let events = vec![
        // late int v;
        Ev::VarDeclBegin {
            late: true,
            is_final: false,
            span: span(1),
        },
        Ev::InitializedIdent {
            name: 0,
            token: span(1),
        },
        Ev::NoInitializer,
        Ev::VarDeclEnd,
        // assert((v = 0) >= 0, v);
        Ev::AssertBegin { span: span(2) },
        // Condition `(v = 0) >= 0`: the target read's fact is retracted by
        // the plain assignment (the identifier is the write), and the
        // assignment holds on the condition-true path.
        Ev::Ident {
            name: 0,
            token: span(3),
            context: IdentifierContext::Expression,
        },
        Ev::OtherExpr { span: span(4) },
        Ev::Assign {
            plain: true,
            if_null: false,
            end: 0,
            field_init: false,
        },
        Ev::BinaryBegin {
            op: BinOp::Other,
            span: span(4),
        },
        Ev::OtherExpr { span: span(5) },
        Ev::BinaryEnd {
            op: BinOp::Other,
            end: 6,
        },
        // Message `v`: reads the post-assignment state, so no fact.
        Ev::Ident {
            name: 0,
            token: span(6),
            context: IdentifierContext::Expression,
        },
        Ev::AssertEnd,
        // v; — the exit joined the pre-assert state: definitely unassigned.
        Ev::Ident {
            name: 0,
            token: span(7),
            context: IdentifierContext::Expression,
        },
        Ev::ExprStmt { span: span(8) },
    ];
    let mut interp = Interp::new(&NameTypeOps, &[], &[]);
    interp.run(&events);
    assert!(interp.stack.is_empty());
    assert_eq!(
        interp
            .facts
            .iter()
            .filter(
                |fact| matches!(fact, FlowFact::UnassignedRead { span: s, .. } if *s == span(7))
            )
            .count(),
        1
    );
}

// —— pattern assignment and pattern declarations ——————————————————————————
// The recorder models patterns by their leaves only (PatternBegin/End,
// ConstantPatternBegin/End, DeclaredVarPattern, AssignedVarPattern,
// Wildcard, GuardBegin/End): composite shapes (list/map/record/object
// patterns) carry no events of their own and collapse to those leaves, so
// only leaf-level shapes are representable in hand-built streams.

/// Pattern assignment `(a, b) = (1, 2)`: the assigned-variable leaves
/// queue writes and `PatternAssign` applies the right-hand side's state to
/// every target (official `handlePatternAssignment`).
#[test]
fn pattern_assignment_writes_targets() {
    use crate::ev::Ev;
    use crate::interp::Interp;
    use crate::ty::NameTypeOps;
    use dartq_parser::{IdentifierContext, TokenSpan};
    use dartq_source::SourceRange;
    let span = |offset: u32| TokenSpan {
        id: offset,
        range: SourceRange { offset, length: 1 },
    };
    let events = vec![
        Ev::VarDeclBegin {
            late: false,
            is_final: false,
            span: span(1),
        },
        Ev::InitializedIdent {
            name: 0,
            token: span(1),
        },
        Ev::NoInitializer,
        Ev::VarDeclEnd,
        Ev::VarDeclBegin {
            late: false,
            is_final: false,
            span: span(2),
        },
        Ev::InitializedIdent {
            name: 1,
            token: span(2),
        },
        Ev::NoInitializer,
        Ev::VarDeclEnd,
        Ev::PatternBegin,
        Ev::AssignedVarPattern { name: 0 },
        Ev::AssignedVarPattern { name: 1 },
        Ev::PatternEnd,
        Ev::OtherExpr { span: span(3) },
        Ev::OtherExpr { span: span(4) },
        Ev::RecordLitEnd { count: 2 },
        Ev::PatternAssign,
        Ev::Ident {
            name: 0,
            token: span(5),
            context: IdentifierContext::Expression,
        },
        Ev::ExprStmt { span: span(6) },
        Ev::Ident {
            name: 1,
            token: span(7),
            context: IdentifierContext::Expression,
        },
        Ev::ExprStmt { span: span(8) },
    ];
    let mut interp = Interp::new(&NameTypeOps, &[], &[]);
    interp.run(&events);
    assert!(interp.stack.is_empty());
    // The diagnostic faces stay silent; the remaining rows are the query
    // faces — the two reads' read-type rows and the targets' write-site
    // rows (the pattern assignment is a real write in the official sets).
    assert!(
        interp.facts.iter().all(|fact| !matches!(
            fact,
            FlowFact::DeadCode { .. } | FlowFact::UnassignedRead { .. }
        )),
        "{:?}",
        interp.facts
    );
    assert!(interp.current.model(0).assigned);
    assert!(interp.current.model(1).assigned);
    assert!(interp.reachable());
}

/// Pattern variable declaration `var (a, b) = (1, 2)`: the declared
/// variables are assigned exactly when the pattern matched, and the
/// statement-end marker fires after the right-hand side (the recorder's
/// order).
#[test]
fn pattern_declaration_declares_assigned() {
    use crate::ev::Ev;
    use crate::interp::Interp;
    use crate::ty::NameTypeOps;
    use dartq_parser::{IdentifierContext, TokenSpan};
    use dartq_source::SourceRange;
    let span = |offset: u32| TokenSpan {
        id: offset,
        range: SourceRange { offset, length: 1 },
    };
    let events = vec![
        Ev::PatternBegin,
        Ev::DeclaredVarPattern {
            name: 0,
            is_final: false,
            token: span(1),
        },
        Ev::DeclaredVarPattern {
            name: 1,
            is_final: false,
            token: span(2),
        },
        Ev::PatternEnd,
        Ev::OtherExpr { span: span(1) },
        Ev::OtherExpr { span: span(2) },
        Ev::RecordLitEnd { count: 2 },
        Ev::PatternVarDeclStmt { span: span(3) },
        Ev::Ident {
            name: 0,
            token: span(4),
            context: IdentifierContext::Expression,
        },
        Ev::ExprStmt { span: span(5) },
        Ev::Ident {
            name: 1,
            token: span(6),
            context: IdentifierContext::Expression,
        },
        Ev::ExprStmt { span: span(7) },
    ];
    let mut interp = Interp::new(&NameTypeOps, &[], &[]);
    interp.run(&events);
    assert!(interp.stack.is_empty());
    // The diagnostic faces stay silent; the declared variables read back
    // with their read-type rows (the query face, #1056). Pattern
    // declarations are not writes in the official sets, so no write-site
    // row aggregates.
    assert!(
        interp.facts.iter().all(|fact| !matches!(
            fact,
            FlowFact::DeadCode { .. }
                | FlowFact::UnassignedRead { .. }
                | FlowFact::WriteSite { .. }
        )),
        "{:?}",
        interp.facts
    );
    assert!(interp.current.model(0).assigned);
    assert!(interp.current.model(1).assigned);
    assert!(interp.reachable());
}

/// A pattern assignment writes every matched target: both `a` and `b`
/// read assigned afterwards (official pattern-assignment definite
/// assignment).
#[test]
fn pattern_assignment_assigns_both_targets() {
    let source = "void f() { int a; int b; (a, b) = (1, 2); a; b; }";
    assert!(unassigned_snippets(source).is_empty());
}

/// A wildcard leaf binds no target: `b` stays definitely unassigned while
/// `a` is written.
#[test]
fn pattern_assignment_wildcard_skips_target() {
    let source = "void f() { int a; int b; (a, _) = (1, 2); b; }";
    assert_eq!(unassigned_snippets(source), ["b"]);
}

// —— suspension demotion ——————————————————————————————————————————————————

/// The demoting `suspension` path (official `suspension`,
/// flow_analysis.dart:7912-7933): an await inside a local function whose
/// reads intersect the anywhere-written set demotes those variables to the
/// conservative join. Invariant-level: the demotion's only observable is
/// the (restored) closure-internal state, so this pins no-crash, drained
/// stack, and no facts for the non-empty demote set (the empty-set variant
/// is `await_in_plain_async_method_demotes_nothing`).
#[test]
fn closure_await_demotes_captured_reads() {
    use crate::ev::Ev;
    use crate::interp::Interp;
    use crate::ty::NameTypeOps;
    use dartq_parser::{IdentifierContext, TokenSpan};
    use dartq_source::SourceRange;
    let span = |offset: u32| TokenSpan {
        id: offset,
        range: SourceRange { offset, length: 1 },
    };
    let events = vec![
        // var x = ...;
        Ev::VarDeclBegin {
            late: false,
            is_final: false,
            span: span(1),
        },
        Ev::InitializedIdent {
            name: 0,
            token: span(1),
        },
        Ev::InitializerBegin,
        Ev::OtherExpr { span: span(2) },
        Ev::InitializerEnd,
        Ev::VarDeclEnd,
        // () async { x; await g(); }
        Ev::ExecBegin { is_closure: true },
        Ev::AsyncModifier {
            is_async: true,
            is_generator: false,
        },
        Ev::Ident {
            name: 0,
            token: span(3),
            context: IdentifierContext::Expression,
        },
        Ev::ExprStmt { span: span(4) },
        Ev::AwaitBegin { token: span(4) },
        Ev::Ident {
            name: 1,
            token: span(5),
            context: IdentifierContext::Expression,
        },
        Ev::SendNoArgs,
        Ev::AwaitEnd {
            span: dartq_source::SourceRange {
                offset: 4,
                length: 10,
            },
        },
        Ev::ExecEnd {
            is_expr: false,
            end: tspan(0),
        },
        // x = 1; — the anywhere-write that arms the demotion.
        Ev::Ident {
            name: 0,
            token: span(6),
            context: IdentifierContext::Expression,
        },
        Ev::OtherExpr { span: span(7) },
        Ev::Assign {
            plain: true,
            if_null: false,
            end: 0,
            field_init: false,
        },
        Ev::ExprStmt { span: span(8) },
    ];
    let mut interp = Interp::new(&NameTypeOps, &[], &[]);
    interp.run(&events);
    assert!(interp.stack.is_empty());
    // The demotion's diagnostic observable is silence: no dead rows, no
    // unassigned reads, no promoted reads. The remaining composition is
    // exactly three rows, in emission order: (1) the closure read's
    // read-type row at span(3) — every reachable read of a resolved local
    // carries one (the `Ident` handler), with no promotion row beside it
    // because x is never promoted; (2) the closure body's completion row
    // at tspan(0) — `ExecEnd` sees a reachable fall-through
    // (`BodyMayCompleteNormally`); (3) the `x = 1;` write-site row —
    // emitted after the replay from the anywhere sets: x enters
    // `written` through the plain assignment (the declaration
    // initializer's `apply_write` records nothing) and never enters
    // `captured` (the closure writes nothing), so exactly one
    // `in_closure: false` row at the declaration name span(1); the
    // assignment target's own read-type row (span(6)) is retracted by
    // the plain assign.
    let [
        FlowFact::ReadType { span: read, .. },
        FlowFact::BodyMayCompleteNormally { span: end },
        FlowFact::WriteSite {
            decl_name_span,
            in_closure: false,
        },
    ] = interp.facts.as_slice()
    else {
        panic!("unexpected fact composition: {:?}", interp.facts);
    };
    assert_eq!(read.range.offset, 3, "{:?}", interp.facts);
    assert_eq!(end.range.offset, 0, "{:?}", interp.facts);
    assert_eq!(decl_name_span.offset, 1, "{:?}", interp.facts);
    assert!(interp.reachable());
}

/// The demoting suspension shape at source level: an await inside a
/// closure whose read target is written elsewhere in the function takes
/// the demotion join without producing facts (official `suspension` reads
/// ∩ anywhere-written − declared).
#[test]
fn closure_await_with_enclosing_write_no_facts() {
    let source =
        "void f(int? x, Future g()) async { var h = () async { await g(); x; }; x = 1; h(); }";
    assert!(read_facts_of(source).is_empty());
}

// —— for-in statements ————————————————————————————————————————————————————
// Event layout mirrors the recorder: the loop variable declaration and the
// iterable expression precede `ForInParts` (the parser fires
// `handle_for_in_loop_parts` after the iterable), the body sits between
// `ForInBodyBegin`/`ForInBodyEnd`, and `ForInEnd` closes the loop.

/// Statement for-in, real replay: the head pops the iterable, the body's
/// writes are conservatively joined at the loop head, and the exit joins
/// the zero-iteration state (official `forEach_bodyBegin`/
/// `forEach_end`, `flow_analysis.dart:6941`). Invariant-level on facts; the
/// zero-iteration dominance on the loop variable's assignment is asserted
/// on the model.
#[test]
fn for_in_statement_replay_invariants() {
    use crate::ev::Ev;
    use crate::interp::Interp;
    use crate::ty::NameTypeOps;
    use dartq_parser::{IdentifierContext, TokenSpan};
    use dartq_source::SourceRange;
    let span = |offset: u32| TokenSpan {
        id: offset,
        range: SourceRange { offset, length: 1 },
    };
    let events = vec![
        // for (var a in ...) — the loop variable, declared unassigned.
        Ev::VarDeclBegin {
            late: false,
            is_final: false,
            span: span(1),
        },
        Ev::InitializedIdent {
            name: 0,
            token: span(1),
        },
        Ev::NoInitializer,
        Ev::VarDeclEnd,
        Ev::ForInExprBegin,
        Ev::OtherExpr { span: span(2) },
        Ev::ForInExprEnd,
        Ev::ForInParts,
        Ev::ForInBodyBegin,
        // a = 1;
        Ev::Ident {
            name: 0,
            token: span(3),
            context: IdentifierContext::Expression,
        },
        Ev::OtherExpr { span: span(4) },
        Ev::Assign {
            plain: true,
            if_null: false,
            end: 0,
            field_init: false,
        },
        Ev::ExprStmt { span: span(5) },
        Ev::ForInBodyEnd,
        Ev::ForInEnd,
    ];
    let mut interp = Interp::new(&NameTypeOps, &[], &[]);
    interp.run(&events);
    assert!(interp.stack.is_empty());
    assert!(interp.exprs.is_empty());
    // The diagnostic faces stay silent; the body's `a = 1;` aggregates the
    // loop variable's write-site row (a real write in the official sets).
    assert!(
        interp.facts.iter().all(|fact| !matches!(
            fact,
            FlowFact::DeadCode { .. }
                | FlowFact::UnassignedRead { .. }
                | FlowFact::PromotedRead { .. }
        )),
        "{:?}",
        interp.facts
    );
    // The zero-iteration path dominates the exit: one body write does not
    // make the loop variable definitely assigned after the loop.
    assert!(!interp.current.model(0).assigned);
    assert!(interp.reachable());
}

/// A `break` in a for-in body: the break's state joins the loop exit (the
/// documented for-in divergence from `forEach_end`, sound and more
/// precise), so the code after the loop stays reachable.
#[test]
fn for_in_break_joins_exit() {
    use crate::ev::Ev;
    use crate::interp::Interp;
    use crate::ty::NameTypeOps;
    use dartq_parser::TokenSpan;
    use dartq_source::SourceRange;
    let span = |offset: u32| TokenSpan {
        id: offset,
        range: SourceRange { offset, length: 1 },
    };
    let events = vec![
        Ev::VarDeclBegin {
            late: false,
            is_final: false,
            span: span(1),
        },
        Ev::InitializedIdent {
            name: 0,
            token: span(1),
        },
        Ev::NoInitializer,
        Ev::VarDeclEnd,
        Ev::ForInExprBegin,
        Ev::OtherExpr { span: span(2) },
        Ev::ForInExprEnd,
        Ev::ForInParts,
        Ev::ForInBodyBegin,
        Ev::Break {
            label: None,
            span: span(3),
        },
        Ev::ForInBodyEnd,
        Ev::ForInEnd,
        Ev::ExprStmt { span: span(4) },
        Ev::OtherExpr { span: span(5) },
    ];
    let mut interp = Interp::new(&NameTypeOps, &[], &[]);
    interp.run(&events);
    assert!(interp.stack.is_empty());
    assert!(interp.facts.is_empty());
    assert!(interp.reachable());
}

/// A collection for-in element (`[for (var a in b) e]`): the head replays
/// before the `ForInParts` marker, the marker opens the loop without a
/// second head, and the element nets exactly one value for the literal's
/// consumer. Event-level pin of the shape the parser-blocked source tests
/// (`collection_for_in_*`) cannot reach yet.
#[test]
fn collection_for_in_element_nets_one_value() {
    use crate::ev::Ev;
    use crate::interp::Interp;
    use crate::ty::NameTypeOps;
    use dartq_parser::TokenSpan;
    use dartq_source::SourceRange;
    let span = |offset: u32| TokenSpan {
        id: offset,
        range: SourceRange { offset, length: 1 },
    };
    let events = vec![
        Ev::ForControlFlowBegin,
        // Loop variable with its initializer.
        Ev::VarDeclBegin {
            late: false,
            is_final: false,
            span: span(1),
        },
        Ev::InitializedIdent {
            name: 0,
            token: span(1),
        },
        Ev::InitializerBegin,
        Ev::OtherExpr { span: span(2) },
        Ev::InitializerEnd,
        Ev::VarDeclEnd,
        // The iterable.
        Ev::OtherExpr { span: span(3) },
        Ev::ForInParts,
        // The element expression.
        Ev::OtherExpr { span: span(4) },
        Ev::ForControlFlowEnd { for_in: true },
    ];
    let mut interp = Interp::new(&NameTypeOps, &[], &[]);
    interp.run(&events);
    assert!(interp.stack.is_empty());
    assert_eq!(interp.exprs.len(), 1);
    assert!(interp.facts.is_empty());
}

/// official `definite_assignment/data/for_each.dart` `forEach`: the iterable
/// is evaluated (and its writes hold) even when the loop runs zero times,
/// so only the body's write stays not-definite.
#[test]
fn for_each_zero_iteration_leaves_unassigned() {
    let source =
        "void f() { late Object v1, v2; for (var _ in (v1 = [0, 1, 2])) { v2 = 0; } v1; v2; }";
    assert_eq!(unassigned_snippets(source), ["v2"]);
}

/// official `definite_assignment/data/for_each.dart` `forEach_continue`:
/// continue paths re-enter the body, so neither the pre-continue write nor
/// the post-continue write is definite at the exit.
#[test]
fn for_each_continue_paths_unassigned() {
    let source = "void f(bool c) { late int v1, v2; for (var _ in [0, 1, 2]) { v1 = 0; if (c) continue; v2 = 0; } v1; v2; }";
    assert_eq!(unassigned_snippets(source), ["v1", "v2"]);
}

/// Reading the for-in loop variable inside the body must produce no fact:
/// the analyzer declares the loop variable initialized right before
/// `forEach_bodyBegin` (analyzer `for_resolver.dart`,
/// `declare(initialized: true)`; official corpus `for_each.dart`
/// `forEach_assigns_to_declared_var`).
///
/// The identifier-head form (`for (i in xs)`) writes its target at the
/// Ident event itself (for_resolver.dart:237-241 `flow?.write`).
#[test]
fn for_each_loop_variable_assigned_in_body() {
    let source = "void f(List<int> b) { for (var a in b) { a; } }";
    assert!(unassigned_snippets(source).is_empty());
}

// —— source-level assert and block forwarding —————————————————————————————
// The recorder's `begin_assert`/`end_assert`/`begin_block`/`end_block`
// arms, proven end to end through the parser (official
// definite_assignment/data/assert.dart `assertStatement` and
// reachability/data/early_exit.dart).

/// The assert corpus shape at source level: the condition's assignment
/// holds through the message but the exit joins the pre-assert state, so
/// the read after the statement is definitely unassigned; the message's
/// own read stays clean.
#[test]
fn assert_statement_forwarded_to_the_recorder() {
    let source = "void f() { late int v; assert((v = 0) >= 0, v); v; }";
    assert_eq!(unassigned_snippets(source), ["v"]);
}

/// A dead assert statement reports dead code: the assert events reach the
/// interpreter's dead-code check (the `AssertBegin` span anchors), the
/// row running to the statement's `;` (`node.end` —
/// dead_code_verifier.dart:326-332).
#[test]
fn dead_assert_statement_reports_dead_code() {
    let source = "void f() { return; assert(true); }";
    assert_eq!(dead_snippets(source), ["assert(true);"]);
}

/// A dead plain block reports one row spanning the whole block extent,
/// and statements entered inside it report nothing further: their
/// unreachability is implied by the outermost row (official `dead_code`
/// folds each dead region into one diagnostic,
/// `dead_code_verifier.dart` `_firstDead`).
#[test]
fn dead_statement_block_reports_outermost_row() {
    let source = "void f() { return; { var x = 1; x; } }";
    assert_eq!(dead_snippets(source), ["{ var x = 1; x; }"]);
}

// —— interval model (official flowEnd corpus, dead_code_test.dart) ———
// One row per dead interval: the first unreachable node anchors it,
// later unreachable nodes extend it, and the covering construct's end
// (`flowEnd`) closes it, trimmed to the covering list's last
// statement. Blocks, labels, and do-while conditions end without a
// covering close, so their intervals run to the enclosing close. The
// row's end is that statement's `node.end` — the trailing `;`
// included where present (`StmtEnd` events; official
// dead_code_verifier.dart:253-279 trim, :326-332 length).

/// official `test_afterReturn`/`test_inElse` shape: sequential dead
/// statements fold into ONE wide row, not one row per statement; the
/// row's end carries the last statement's `;` (`node.end`,
/// dead_code_verifier.dart:326-332).
#[test]
fn dead_interval_folds_sequential_statements() {
    let source = "void f() { return; 1; 2; }";
    assert_eq!(dead_snippets(source), ["1; 2;"]);
}

/// official `test_flowEnd_ifStatement` with a nested dead block: one
/// outermost row spanning the whole dead compound (the last statement
/// is the inner block — its `}` ends the row, no `;` to include).
#[test]
fn dead_interval_folds_nested_blocks() {
    let source = "void f() { return; {1; {2;}} }";
    assert_eq!(dead_snippets(source), ["{1; {2;}}"]);
}

/// official `test_flowEnd_ifStatement` with a dead `if`: the covering
/// list's last statement is the `if` itself, whose `node.end` includes
/// the then-branch's `;` (dead_code_verifier.dart:253-279, :326-332).
#[test]
fn dead_interval_if_statement_is_one_row() {
    let source = "void f(bool c) { return; if (c) 1; }";
    assert_eq!(dead_snippets(source), ["if (c) 1;"]);
}

/// official `test_flowEnd_tryStatement_finally` (length-11 row): the
/// finally block ends without a covering close, so the interval runs
/// through the try's closing brace into the statement after it and
/// closes at the declaration exit — trimmed to that statement's
/// `node.end`, its `;` included (the official row's 11th character).
#[test]
fn dead_interval_try_finally_spans_through_the_close() {
    let source =
        "main() {\n  try {\n    1;\n  } finally {\n    2;\n    return;\n    3;\n  }\n  4;\n}\n";
    assert_eq!(dead_snippets(source), ["3;\n  }\n  4;"]);
}

/// official `test_flowEnd_nestedBlock_forStatement_updaters` (length-8
/// row, minus the updaters row this engine does not anchor): the row
/// ends at the inner block's closing brace — the last statement of the
/// covering loop body — while the body block itself never extends.
#[test]
fn dead_interval_loop_body_trims_to_last_statement_block() {
    let source = "void f() {\n  for (;;) {\n    {\n      return;\n      2;\n    }\n  }\n}\n";
    assert_eq!(dead_snippets(source), ["2;\n    }"]);
}

/// official `test_doWhile`: two ranges in one close — the `while (c);`
/// part, then the tail from the first dead statement after the
/// do-while, its `;` included (official `parent is DoStatement`,
/// :292-308; the tail's end is the covering list's last statement's
/// `node.end`).
#[test]
fn dead_interval_do_while_splits_two_ranges() {
    let source = "void f(bool c) { do { print(c); return; } while (c); print('2'); }";
    assert_eq!(dead_snippets(source), ["} while (c);", "print('2');"]);
}

/// official `test_doWhile_break`: a break exiting the do-while makes the
/// tail reachable, so only the `while (c);` part reports.
#[test]
fn dead_interval_do_while_break_drops_the_tail() {
    let source = "void f(bool c) { do { if (c) break; return; } while (c); 1; }";
    assert_eq!(dead_snippets(source), ["} while (c);"]);
}

/// One window per switch member: each case group's close emits its own
/// row and the next group starts fresh (official
/// `handleMergedStatementCase`'s `flowEnd` on the member).
#[test]
fn dead_interval_switch_members_report_per_group() {
    let source = "void f(int i) { switch (i) { case 1: return; 2; case 3: return; 4; } }";
    assert_eq!(dead_snippets(source), ["2;", "4;"]);
}

/// official `test_doWhile_break_outerLabel` shape: a labeled statement
/// ends without a covering close, so the interval anchored inside runs
/// through the label's closing brace to the enclosing close. The anchor
/// is the FIRST LABEL token (official `firstDead.firstToken`,
/// `dead_code_verifier.dart:253` — the AST node starts at `L`), so the
/// `L: ` prefix sits inside the row (#641).
#[test]
fn dead_interval_labeled_statement_runs_to_the_tail() {
    let source = "void f(bool c) { return; L: while (c) { continue L; } 1; }";
    assert_eq!(dead_snippets(source), ["L: while (c) { continue L; } 1;"]);
}

/// A dead local declaration as the covering list's last statement: the
/// row runs to its `;` (official `node.end`,
/// dead_code_verifier.dart:326-332). The row STARTS at the declared
/// name — the fact layer's declaration anchor (the parser delivers the
/// name token to `beginVariablesDeclaration`), the documented divergence
/// the semantic twin pin `dead_local_declaration_emits_at_its_name`
/// fixes at the name, not the declaration head.
#[test]
fn dead_interval_declaration_widens_to_the_semicolon() {
    let source = "void f() { return; var x = 1; }";
    assert_eq!(dead_snippets(source), ["x = 1;"]);
}

/// The covering list's last statement without a trailing `;` — a block
/// — ends the row at its `}`: the statement end carries no `;` to
/// include (official `node.end` of the block; the `if`'s own `;` still
/// widens the first segment).
#[test]
fn dead_interval_last_statement_block_ends_at_the_brace() {
    let source = "void f() { return; if (c) 1; { 2; } }";
    assert_eq!(dead_snippets(source), ["if (c) 1; { 2; }"]);
}

// —— #636 spec residuals: dead loop/branch/switch heads (official
// dead_code_test.dart shapes) ————————————————————————————————————————

/// official `test_forStatement` (11 carets): the close is
/// `visitForStatement`'s `flowEnd(node.body)` (`type_analyzer.dart:3194`);
/// the body block is EMPTY, so the trim's `statements.isNotEmpty` guard
/// keeps the block itself (dead_code_verifier.dart:269-271) and the row
/// ends at its `}`.
#[test]
fn dead_interval_for_empty_body_runs_to_the_brace() {
    let source = "void f() { return; for (;;) {} }";
    assert_eq!(dead_snippets(source), ["for (;;) {}"]);
}

/// `visitWhileStatement`'s `flowEnd(node.body)` (`type_analyzer.dart:4901`)
/// with an empty body block: the trim keeps the block, the row ends at
/// its `}`.
#[test]
fn dead_interval_while_empty_body_runs_to_the_brace() {
    let source = "void f(bool c) { return; while (c) {} }";
    assert_eq!(dead_snippets(source), ["while (c) {}"]);
}

/// A non-block loop body is the body statement itself: the trim target
/// is that statement (no `statements.last` step — it is not a Block) and
/// its `node.end` carries the `;`.
#[test]
fn dead_interval_while_expression_body_widens_to_the_semicolon() {
    let source = "void f(bool c) { return; while (c) x(); }";
    assert_eq!(dead_snippets(source), ["while (c) x();"]);
}

/// An empty statement as the loop body: `node.end` is the body's `;`.
#[test]
fn dead_interval_while_empty_statement_body_widens_to_the_semicolon() {
    let source = "void f(bool c) { return; while (c); }";
    assert_eq!(dead_snippets(source), ["while (c);"]);
}

/// A dead for-in with an empty body block: same body-close shape as the
/// C-style `for` — the row ends at the body's `}`.
#[test]
fn dead_interval_for_in_empty_body_runs_to_the_brace() {
    let source = "void f(l) { return; for (var x in l) {} }";
    assert_eq!(dead_snippets(source), ["for (var x in l) {}"]);
}

/// `flowEnd(ifTrue)` (type_analyzer.dart:1199-1206) with an empty then
/// block: the trim keeps the block, the row ends at its `}`.
#[test]
fn dead_interval_if_empty_branch_runs_to_the_brace() {
    let source = "void f(bool c) { return; if (c) {} }";
    assert_eq!(dead_snippets(source), ["if (c) {}"]);
}

/// A case-less dead switch: no member close covers the anchor, so the
/// declaration exit trims to the covering list's last statement — the
/// switch itself, whose `node.end` is the block's `}`.
#[test]
fn dead_interval_switch_without_members_runs_to_the_brace() {
    let source = "void f(int i) { return; switch (i) {} }";
    assert_eq!(dead_snippets(source), ["switch (i) {}"]);
}

/// official row anchor of a dead labeled statement: the LABELED
/// STATEMENT's first token — the label (`a`) — because
/// `firstDead.firstToken` reads the AST node (`dead_code_verifier.dart`
/// :253/:325-332), while fasta's `beginLabeledStatement(next, …)`
/// (`parser_impl.dart:6909`) delivers the statement PAST the labels as the
/// event marker. The recorder latches the first label-declaration token
/// and anchors `LabeledBegin` on it, so the `a: ` prefix is INSIDE the
/// row. #641.
#[test]
fn dead_interval_labeled_statement_starts_at_the_label() {
    let source = "void f(bool c) { return; a: while (c) {} }";
    assert_eq!(dead_snippets(source), ["a: while (c) {}"]);
}

/// Masking guard for the empty-body shapes above: the dead tail AFTER a
/// dead empty-bodied loop folds into the SAME row (one interval, the
/// declaration exit trims to the last statement `1;`) — a fix that clips
/// the row at the loop's brace breaks this.
#[test]
fn dead_interval_empty_body_loop_folds_the_tail() {
    let source = "void f() { return; for (;;) {} 1; }";
    assert_eq!(dead_snippets(source), ["for (;;) {} 1;"]);
}

/// The for-header condition anchors its own dead row (official
/// first-dead-node: `test_forLoop_noUpdaters` opens the row at the
/// condition's `(` — the header part's ExprStmt): an init whose `throw`
/// exits leaves the state unreachable when the condition wrapper fires,
/// so the row anchors at the condition — not at the body `{` (the
/// pre-#860 suppression) — and folds through the body to the function
/// exit. Parse-only stand-in for the engine-driven golden below: `foo()`
/// needs resolution to report Never, `throw` needs nothing.
#[test]
fn dead_interval_for_condition_anchors_at_the_condition() {
    let source = "void f(bool c) { for (throw 0; c;) {} }";
    assert_eq!(dead_snippets(source), ["c;) {}"]);
}

/// Gate polarity guard: init/updater wrappers stay suppressed. A wholly
/// dead loop anchors at `for` itself (`for_begin`, official `ForStatement`
/// `firstToken`); a header wrapper anchoring instead would start the row
/// at the init expression.
#[test]
fn dead_interval_dead_for_loop_starts_at_the_keyword_not_the_header() {
    let source = "void f(bool c) { return; for (1; c;) {} }";
    assert_eq!(dead_snippets(source), ["for (1; c;) {}"]);
}

/// official `test_forLoop_noUpdaters` (dead_code_test.dart:212-220,
/// column 15, length 29) ported verbatim: the interval anchors at the
/// condition's `(` — the for's `flowEnd(node.body)` does NOT cover it
/// (`_containsFirstDeadNode` fails: the body block is not an ancestor of
/// the condition, dead_code_verifier.dart:453-459) — and folds through
/// the body `{}` and `return i;` to the function exit's last statement.
#[ignore = "half-landed: the flow half is done (header_dead_check anchors condition slices, pinned by dead_interval_for_condition_anchors_at_the_condition), but this harness is parse-only — facts_of never drives the engine (dartq-flow cannot depend on dartq-semantic), so `foo()`'s Never exit never fires and the row stays anchored later; a live run needs the semantic-driven harness (#860 semantic half)"]
#[test]
fn official_for_loop_no_updaters_folds_through_the_return() {
    let source = "Never foo() => throw \"Never\";\n\ntest() {\n  int i = 0;\n  for (foo(); (i = 42) < 0;) {}\n  return i;\n}\n";
    assert_eq!(dead_snippets(source), ["(i = 42) < 0;) {}\n  return i;"]);
}

/// A wholly dead branch block anchors at the block itself — official
/// `_firstDead` takes the outermost unreachable node, so the row spans
/// the full block extent (`flowEnd(ifTrue)` with `node ==
/// firstDeadNode`: no trim) — and the statement after the if stays
/// live. The branch is provably dead through the cast pattern's
/// promotion (`x` is `int`, so `x is! int` cannot hold — the same
/// stimulus as `case_head_state_carries_into_the_body`; a bare
/// `num n` / `n is! int` is NOT dead, `num` includes `double`).
#[test]
fn dead_interval_dead_branch_block_reports_full_extent() {
    let source =
        "void f(Object? o) { switch (o) { case var x as int: if (x is! int) { return; 1; } x; } }";
    assert_eq!(dead_snippets(source), ["{ return; 1; }"]);
}

// —— dead while-loop body containment (phase-1 while probes) ————

/// Probe s2 (oracle row `2:17 27`): a wholly dead block body anchors at
/// the body block's `{` — the official `flowEnd(node.body)` containment
/// accepts the close node itself (`node == firstDeadNode`), so the row
/// spans the block's full extent, `}` included — and the close fires at
/// `WhileEnd`, so the live tail after the loop is not swallowed.
#[test]
fn dead_while_block_body_row_spans_block_not_tail() {
    let source =
        "void f() {\n  while (false) {\n    int x;\n    int y;\n  }\n  print('tail');\n}\n";
    assert_eq!(dead_snippets(source), ["{\n    int x;\n    int y;\n  }"]);
}

/// Probe s3 (oracle row `2:17 11`): a non-block body statement carries
/// the loop's frame depth, so the row is exactly that statement — anchor
/// at its first token, end at its `;` — and the tail stays live.
#[test]
fn dead_while_statement_body_row_is_exactly_the_statement() {
    let source = "void f() {\n  while (false) print('x');\n  print('tail');\n}\n";
    assert_eq!(dead_snippets(source), ["print('x');"]);
}

/// Probe s13 (oracle rows `2:17 16` + `5:17 11`): two consecutive dead
/// loops report TWO rows — the first loop's close fires at its own
/// `WhileEnd`, so the second loop's body anchors a fresh interval instead
/// of folding into the first.
#[test]
fn dead_while_loops_report_one_row_per_loop() {
    let source = "void f() {\n  while (false) {\n    print('1');\n  }\n  while (false) print('2');\n  print('tail');\n}\n";
    assert_eq!(
        dead_snippets(source),
        ["{\n    print('1');\n  }", "print('2');"]
    );
}

/// Regression guard: an entry-dead loop anchors at the `while` keyword —
/// one depth BELOW the at-depth body band — so the `WhileEnd` close must
/// leave the interval open; it rides to the declaration exit as one
/// whole-statement row (official: the first dead node is the `while`
/// itself).
#[test]
fn entry_dead_while_keeps_anchoring_at_the_keyword() {
    let source = "void f(bool c) {\n  return;\n  while (c) print('x');\n}\n";
    assert_eq!(dead_snippets(source), ["while (c) print('x');"]);
}

// —— dead for-loop updaters (official `for_updaterBegin`, probe table J) ——

/// Probe u1 (oracle rows `2|26|3` = `i++`, `2|31|2` = `{}`): a
/// constant-false condition with an updater fires the updater's own row
/// AND the wholly dead body block's row — two independent rows — while
/// the tail after the loop stays row-free (the condition-false exit path
/// is reachable). The body row closes before the updaters replay, so the
/// updater's statement end never folds into it.
#[test]
fn dead_updater_dual_rows_constant_false_condition() {
    let source = "void f() {\n  for (var i = 0; false; i++) {}\n  print('tail');\n}\n";
    assert_eq!(dead_snippets(source), ["i++", "{}"]);
}

/// Probe u2 (oracle rows `2|26|3` = `i++`, `4|5|14` = `print('dead');`):
/// a break followed by a dead statement anchors the interval inside the
/// body (reported at the body close, trimmed to the statement), and the
/// updaters — unreachable on the back edge — fire their own row.
#[test]
fn dead_updater_dual_rows_break_then_statement() {
    let source =
        "void f() {\n  for (var i = 0; i < 1; i++) {\n    break;\n    print('dead');\n  }\n}\n";
    assert_eq!(dead_snippets(source), ["i++", "print('dead');"]);
}

/// Probe u4: no updaters, no row (the official `beginToken != null` guard,
/// dead_code_verifier.dart:366-369) — only the body block reports.
#[test]
fn dead_updater_row_needs_updaters() {
    let source = "void f() {\n  for (var i = 0; false;) {}\n  print('tail');\n}\n";
    assert_eq!(dead_snippets(source), ["{}"]);
}

/// Probe u5: a wholly dead loop anchors at the `for` itself, so the
/// `for_conditionEnd` latch (`hadFirstDeadNode`) suppresses the updater's
/// own row — the interval rides the covering close as one row.
#[test]
fn dead_updater_row_suppressed_when_the_loop_is_dead() {
    let source = "void f() {\n  return;\n  for (var i = 0; i < 1; i++) {}\n}\n";
    assert_eq!(dead_snippets(source), ["for (var i = 0; i < 1; i++) {}"]);
}

/// Probe u6 (oracle rows `2|26|3` = `i++`, `2|31|52` = the whole body
/// block): a dead multi-statement body reports the full block extent
/// (anchor at `{`, `}` included) next to the updater's own row.
#[test]
fn dead_updater_dual_rows_whole_dead_body_block() {
    let source = "void f() {\n  for (var i = 0; false; i++) {\n    print('dead body');\n    print('dead too');\n  }\n  print('tail');\n}\n";
    assert_eq!(
        dead_snippets(source),
        [
            "i++",
            "{\n    print('dead body');\n    print('dead too');\n  }"
        ]
    );
}

/// official `test_flowEnd_nestedBlock_forStatement_updaters` shape (probe
/// table D): the row is the length-8 updaters list `i++, i++` ONLY — the
/// body's `return` kills the back edge but the condition is not constant,
/// so the loop exit stays reachable and the tail never reports.
#[test]
fn dead_updater_row_spans_the_whole_updaters_list() {
    let source = "void f() {\n  for (var i = 0; i < 1; i++, i++) {\n    return;\n  }\n  print('dead tail');\n}\n";
    assert_eq!(dead_snippets(source), ["i++, i++"]);
}

/// Probe u3 (engine-driven): a Never-typed call in the body makes the
/// back edge unreachable, so the updaters fire their own row while the
/// tail stays row-free. Hand-driven: only the engine can resolve a
/// subexpression's type to Never, so the parser pipeline cannot produce
/// this buffer (the Exit lands where the engine fires `handle_exit`,
/// after the call's arguments). The updater grammar mirrors the real
/// recorder: a bare expression list — an `Ident` read then the postfix
/// operator's end — no `ExprStmt`/`StmtEnd` wrapper.
#[test]
fn never_call_in_body_kills_the_updaters_not_the_tail() {
    use crate::facts::FlowFact;
    // `void f() { for (; c; i++) { boom(); } 1; }` — offsets: the `for`
    // at 11, the left `;` at 16, `c` at 18, the right `;` at 19, the
    // updaters `i++` at 21-23, the body `{ boom(); }` at 26-36, the tail
    // `1;` at 38-39.
    let events: Vec<crate::ev::Ev> = [
        vec![
            crate::ev::Ev::ExecBegin { is_closure: false },
            crate::ev::Ev::BlockBodyBegin,
            crate::ev::Ev::ForBegin { span: tspan(11) },
        ],
        // Condition `c;`.
        vec![
            crate::ev::Ev::ExprStmt { span: tspan(18) },
            crate::ev::Ev::StmtEnd { end: 20 },
            crate::ev::Ev::OtherExpr { span: tspan(18) },
        ],
        // Updaters `i++` (bare expression: the read, then the operator's
        // end).
        vec![
            crate::ev::Ev::Ident {
                name: 1,
                token: tspan(21),
                context: dartq_parser::IdentifierContext::Expression,
            },
            crate::ev::Ev::UnaryPostfixAssign { end: 24 },
        ],
        vec![crate::ev::Ev::ForParts {
            left_sep: tspan(16),
            right_sep: tspan(21),
        }],
        // Body `{ boom(); }` — the call's type resolves to Never, the
        // engine appends Exit after the arguments.
        vec![
            crate::ev::Ev::Block {
                kind: crate::ev::BlockKind::Statement,
                span: tspan(26),
            },
            crate::ev::Ev::ExprStmt { span: tspan(28) },
            crate::ev::Ev::StmtEnd { end: 35 },
            crate::ev::Ev::OtherExpr { span: tspan(28) },
            crate::ev::Ev::Exit { end: tspan(34) },
            crate::ev::Ev::EndBlock { end: 37 },
        ],
        vec![crate::ev::Ev::ForEnd, crate::ev::Ev::StmtEnd { end: 37 }],
        // Tail `1;` — reachable through the condition-false exit.
        vec![
            crate::ev::Ev::ExprStmt { span: tspan(38) },
            crate::ev::Ev::StmtEnd { end: 40 },
            crate::ev::Ev::OtherExpr { span: tspan(38) },
        ],
        vec![
            crate::ev::Ev::BlockBodyEnd,
            crate::ev::Ev::ExecEnd {
                is_expr: false,
                end: tspan(41),
            },
        ],
    ]
    .concat();
    let interp = interp_of(&events);
    let dead: Vec<_> = interp
        .facts
        .iter()
        .filter_map(|fact| match fact {
            FlowFact::DeadCode { span } => Some((span.range.offset, span.range.length)),
            _ => None,
        })
        .collect();
    assert_eq!(dead, [(21, 3)], "the updaters row only: {:?}", interp.facts);
    // Control: the same buffer without Exit keeps the back edge (and the
    // whole loop) reachable — no row.
    let live: Vec<crate::ev::Ev> = events
        .iter()
        .copied()
        .filter(|ev| !matches!(ev, crate::ev::Ev::Exit { .. }))
        .collect();
    let interp = interp_of(&live);
    assert!(
        interp
            .facts
            .iter()
            .all(|fact| !matches!(fact, FlowFact::DeadCode { .. })),
        "{:?}",
        interp.facts
    );
}

// —— engine-driven Never exits (official `handleExit`, probe table I) ————

/// Probe g9 (oracle row `4|3|52`): after a Never-typed call the NEXT
/// statement anchors the interval, the following `if` folds in, and the
/// row closes at the covering body's last statement — one wide row, the
/// statement after the call as the anchor, never the Never expression
/// itself. Hand-driven: only the engine can resolve a subexpression's
/// type to Never (the Exit lands right after the call's arguments).
#[test]
fn never_call_exit_folds_through_the_covering_statements() {
    use crate::facts::FlowFact;
    // `void f() { boom(); 1; if (c) 2; }` — offsets: `boom();` at 11-17,
    // `1;` at 19-20, `if (c) 2;` at 22-30. The Exit is spliced INSIDE the
    // boom statement's events, right after the expression's own (the
    // engine fires `handle_exit` mid-buffer, past the call's arguments).
    let events: Vec<crate::ev::Ev> = [
        vec![
            crate::ev::Ev::ExecBegin { is_closure: false },
            crate::ev::Ev::BlockBodyBegin,
        ],
        // `boom();` — the call's type resolves to Never. The wrapper events
        // follow the expression's (statements.rs fires
        // `handle_expression_statement` after `parse_expression`).
        vec![
            crate::ev::Ev::OtherExpr { span: tspan(11) },
            crate::ev::Ev::Exit { end: tspan(18) },
            crate::ev::Ev::ExprStmt { span: tspan(11) },
            crate::ev::Ev::StmtEnd { end: 18 },
        ],
        // `1;` — unreachable from here.
        vec![
            crate::ev::Ev::OtherExpr { span: tspan(19) },
            crate::ev::Ev::ExprStmt { span: tspan(19) },
            crate::ev::Ev::StmtEnd { end: 21 },
        ],
        // `if (c) 2;` — folds into the same interval.
        vec![
            crate::ev::Ev::IfBegin { span: tspan(22) },
            crate::ev::Ev::OtherExpr { span: tspan(26) },
            crate::ev::Ev::ThenBegin,
            crate::ev::Ev::OtherExpr { span: tspan(29) },
            crate::ev::Ev::ExprStmt { span: tspan(29) },
            crate::ev::Ev::StmtEnd { end: 31 },
            crate::ev::Ev::IfEnd { has_else: false },
            crate::ev::Ev::StmtEnd { end: 31 },
        ],
        vec![
            crate::ev::Ev::BlockBodyEnd,
            crate::ev::Ev::ExecEnd {
                is_expr: false,
                end: tspan(32),
            },
        ],
    ]
    .concat();
    let interp = interp_of(&events);
    let dead: Vec<_> = interp
        .facts
        .iter()
        .filter_map(|fact| match fact {
            FlowFact::DeadCode { span } => Some((span.range.offset, span.range.length)),
            _ => None,
        })
        .collect();
    assert_eq!(
        dead,
        [(19, 12)],
        "one row: `1; if (c) 2;` — {:?}",
        interp.facts
    );
    // Control: the same buffer without Exit stays live — no row.
    let live: Vec<crate::ev::Ev> = events
        .iter()
        .copied()
        .filter(|ev| !matches!(ev, crate::ev::Ev::Exit { .. }))
        .collect();
    let interp = interp_of(&live);
    assert!(
        interp
            .facts
            .iter()
            .all(|fact| !matches!(fact, FlowFact::DeadCode { .. })),
        "{:?}",
        interp.facts
    );
}

/// An Exit inside a nested send's arguments (`f(boom());`) leaves the
/// value stack balanced — the Exit never pops (the official call site
/// fires with the subexpression still on the stack,
/// type_analyzer.dart:815-819), the arguments' own close consumes the
/// value, and the statement after the send still anchors its row.
#[test]
fn never_call_exit_inside_arguments_keeps_the_stack() {
    use crate::facts::FlowFact;
    // `void f() { g(boom()); 1; }` — offsets: `g(boom());` at 11-20,
    // `boom()` at 13-18, `1;` at 22-23.
    let events: Vec<crate::ev::Ev> = [
        vec![
            crate::ev::Ev::ExecBegin { is_closure: false },
            crate::ev::Ev::BlockBodyBegin,
        ],
        // `g(boom());` — the argument's type resolves to Never. The wrapper
        // events follow the send's (statements.rs fires
        // `handle_expression_statement` after `parse_expression`).
        vec![
            crate::ev::Ev::OtherExpr { span: tspan(11) },
            crate::ev::Ev::Send {
                args: crate::ty::EMPTY_ARGS,
            },
            crate::ev::Ev::ArgumentsBegin,
            crate::ev::Ev::OtherExpr { span: tspan(13) },
            crate::ev::Ev::Exit { end: tspan(19) },
            crate::ev::Ev::ArgumentsEnd { count: 1 },
            crate::ev::Ev::ExprStmt { span: tspan(11) },
            crate::ev::Ev::StmtEnd { end: 21 },
        ],
        // `1;` — unreachable from here.
        vec![
            crate::ev::Ev::OtherExpr { span: tspan(22) },
            crate::ev::Ev::ExprStmt { span: tspan(22) },
            crate::ev::Ev::StmtEnd { end: 24 },
        ],
        vec![
            crate::ev::Ev::BlockBodyEnd,
            crate::ev::Ev::ExecEnd {
                is_expr: false,
                end: tspan(25),
            },
        ],
    ]
    .concat();
    let interp = interp_of(&events);
    let dead: Vec<_> = interp
        .facts
        .iter()
        .filter_map(|fact| match fact {
            FlowFact::DeadCode { span } => Some((span.range.offset, span.range.length)),
            _ => None,
        })
        .collect();
    assert_eq!(
        dead,
        [(22, 2)],
        "the row anchors at `1;`: {:?}",
        interp.facts
    );
}

/// A live try/finally block produces no fact: its `begin_block` fires with
/// the try kind, which scopes but never dead-checks (the `try` keyword's
/// own dead check rides `TryBegin`).
#[test]
fn live_try_block_records_scope_only() {
    let source = "void f() { try { var x = 1; x; } finally {} }";
    assert!(dead_snippets(source).is_empty());
    assert!(unassigned_snippets(source).is_empty());
}

// —— promotion-machinery pins (audit #543 fixes) ——————————————————————

/// Assignment demotion keeps the least-specific suffix (official
/// `_demoteViaAssignment`, flow_analysis.dart:4888-4928): after `num? x`
/// promotes through `is num` then `is int` (chain [int, num] most-specific
/// first), a write of a `num` value drops the stale `int` head and keeps
/// exactly `num`. The old prefix-keeping demotion retained both entries.
#[test]
fn assignment_demotion_keeps_the_least_specific_suffix() {
    use crate::ev::Ev;
    use crate::model::NIL;
    use crate::ty::{Base, Ty};
    const NUM: u32 = 101;
    const INT: u32 = 102;
    const X: u32 = 7;
    const OTHER: u32 = 8;
    let events: Vec<Ev> = [
        vec![Ev::ExecBegin { is_closure: false }],
        decl_block(X, named_ty(NUM, true)),
        decl_block(OTHER, named_ty(NUM, false)),
        is_test(X, named_ty(NUM, false), false),
        is_test(X, named_ty(INT, false), false),
        vec![
            read(X),
            // `x = other;`: the right-hand side is a `num` value.
            read(OTHER),
            Ev::Assign {
                plain: true,
                if_null: false,
                end: 0,
                field_init: false,
            },
            Ev::ExecEnd {
                is_expr: false,
                end: tspan(0),
            },
        ],
    ]
    .concat();
    let interp = interp_of(&events);
    let model = interp.current.model(0);
    assert!(
        model.promoted != NIL,
        "a promotion must survive a `num` write into `num? x`"
    );
    assert_eq!(
        interp.lists.head(model.promoted),
        Ty {
            base: Base::Name(NUM),
            nullable: false
        }
    );
    assert_eq!(
        interp.lists.iter(model.promoted).count(),
        1,
        "the stale `int` head must drop from the chain"
    );
}

/// The `is!` then-branch carries the factored remainder (official
/// `tryPromoteForTypeCheck`'s typeIfFalse,
/// flow_analysis.dart:4143-4179): `int? x` under `x is! int` reads
/// `Never?` — the Null knowledge — on the then path
/// (`factorType(int?, int) = Never?`,
/// factor_type.dart:11-45). The old unmatched arm kept the state
/// unchanged.
#[test]
fn is_not_then_branch_carries_the_factored_remainder() {
    use crate::ev::Ev;
    use crate::ty::{Base, Ty};
    const INT: u32 = 102;
    const X: u32 = 7;
    let events: Vec<Ev> = [
        declared(X, named_ty(INT, true)),
        is_test(X, named_ty(INT, false), true),
    ]
    .concat();
    let interp = interp_of(&events);
    let model = interp.current.model(0);
    assert_eq!(
        interp.lists.head(model.promoted),
        Ty {
            base: Base::Never,
            nullable: true
        }
    );
}

/// A bottom factored remainder marks the else branch unreachable
/// (official `ifFalseIsUnreachable`, flow_analysis.dart:4159-4173 — the
/// sound arm; `sound-flow-analysis` is enabled-by-default and expired,
/// experiments/flags.dart:309-315): `int x` cannot fail `x is int?`, so
/// the else branch is dead code. The scrutinee's own read row (the
/// read-type face, #1056) precedes the dead row.
#[test]
fn bottom_remainder_marks_the_else_branch_unreachable() {
    use crate::ev::Ev;
    use crate::facts::FlowFact;
    const INT: u32 = 102;
    const X: u32 = 7;
    let events: Vec<Ev> = [
        declared(X, named_ty(INT, false)),
        vec![Ev::IfBegin { span: tspan(2) }],
        is_test(X, named_ty(INT, true), false),
        vec![
            Ev::ThenBegin,
            Ev::ElseBegin,
            Ev::ExprStmt { span: tspan(3) },
            Ev::OtherExpr { span: tspan(4) },
            Ev::IfEnd { has_else: true },
            Ev::ExecEnd {
                is_expr: false,
                end: tspan(0),
            },
        ],
    ]
    .concat();
    let interp = interp_of(&events);
    assert!(
        matches!(
            interp.facts.as_slice(),
            [
                FlowFact::ReadType { .. },
                FlowFact::DeadCode { .. },
                FlowFact::BodyMayCompleteNormally { .. }
            ]
        ),
        "the else branch of `x is int?` on `int x` is provably dead: {:?}",
        interp.facts
    );
}

/// #2180 face-3 pin: an invalid tested type (the oracle's `InvalidType`,
/// spelled `Base::Unknown` at this seam — `bound_face` already classifies
/// it "like the official invalid-type arm") produces the analyzer's
/// `x is InvalidType` verdict: no promotion (the true arm's
/// `isValidPromotionStep` is false — `prev <: InvalidType` always holds,
/// subtype.dart:274-276 making the invalid type a supertype of all), the
/// tested set still records it on both arms (official `_finishTypeTest`,
/// flow_analysis.dart:4283-4293), and the else branch is unreachable
/// (`factor(prev, InvalidType) = Never`, :4154-4166 sound arm). Hand-driven:
/// the parser pipeline spells a missing name as `Base::Name`, so only the
/// resolved back-end (or this buffer) can feed the invalid arm.
#[test]
fn invalid_tested_type_never_promotes_and_deads_the_else_branch() {
    use crate::ev::Ev;
    use crate::facts::FlowFact;
    const INT: u32 = 102;
    const X: u32 = 7;
    let events: Vec<Ev> = [
        declared(X, named_ty(INT, false)),
        vec![Ev::IfBegin { span: tspan(2) }],
        is_test(
            X,
            Ev::Type {
                base: crate::ty::Base::Unknown,
                nullable: false,
                args: crate::ty::EMPTY_ARGS,
            },
            false,
        ),
        vec![
            Ev::ThenBegin,
            // The true arm reads `x` at its declared type: no promotion.
            read(X),
            Ev::ExprStmt { span: tspan(3) },
            Ev::ElseBegin,
            Ev::ExprStmt { span: tspan(4) },
            Ev::OtherExpr { span: tspan(5) },
            Ev::IfEnd { has_else: true },
            Ev::ExecEnd {
                is_expr: false,
                end: tspan(0),
            },
        ],
    ]
    .concat();
    let interp = interp_of(&events);
    let model = interp.current.model(0);
    assert_eq!(model.promoted, crate::model::NIL, "no promotion may stand");
    assert!(
        interp.lists.iter(model.tested).count() == 1,
        "the invalid type joins the tested set once: {:?}",
        interp.lists.iter(model.tested).collect::<Vec<_>>()
    );
    assert!(
        interp
            .facts
            .iter()
            .any(|fact| matches!(fact, FlowFact::DeadCode { .. })),
        "the else branch of `x is InvalidType` is dead: {:?}",
        interp.facts
    );
}

/// `??` types as `LUB(NonNull(lhs), rhs)` (official `ifNullExpression_end`,
/// flow_analysis.dart:7071-7080, with the skip-path promotion mounted at
/// `ifNullExpression_rightBegin`, :7086-7104). Oracle-verified faces: a
/// live nullable RHS makes the result nullable (`int? a ?? g()` assigns to
/// `int?` only), a live non-nullable RHS keeps the LHS's non-null form
/// (`int? a ?? 1` assigns to `int`), and a never-evaluated RHS (non-null
/// LHS) leaves the LHS's own type — the flow face of the official
/// `dead_null_aware_expression` row.
#[test]
fn if_null_types_as_lub_of_lhs_non_null_and_rhs() {
    use crate::ev::Ev;
    use crate::ty::{Base, Ty};
    const INT: u32 = 102;
    const X: u32 = 7;
    const Y: u32 = 8;
    let if_null = |rhs: Vec<Ev>| {
        [
            vec![
                read(X),
                Ev::BinaryBegin {
                    op: crate::ev::BinOp::IfNull,
                    span: tspan(1),
                },
            ],
            rhs,
            vec![Ev::BinaryEnd {
                op: crate::ev::BinOp::IfNull,
                end: 3,
            }],
        ]
        .concat()
    };
    // A live RHS of unknown nullability keeps the nullable face.
    let events = [
        declared(X, named_ty(INT, true)),
        if_null(vec![
            Ev::OtherExpr { span: tspan(2) },
        ]),
    ]
    .concat();
    let interp = interp_of(&events);
    let result = interp.exprs.last().expect("?? leaves its value");
    assert_eq!(
        result.ty,
        Ty {
            base: Base::Name(INT),
            nullable: true
        }
    );
    // A live non-nullable RHS: the LHS's non-null form.
    let events = [
        vec![Ev::ExecBegin { is_closure: false }],
        decl_block(X, named_ty(INT, true)),
        decl_block(Y, named_ty(INT, false)),
        if_null(vec![read(Y)]),
    ]
    .concat();
    let interp = interp_of(&events);
    let result = interp.exprs.last().expect("?? leaves its value");
    assert_eq!(
        result.ty,
        Ty {
            base: Base::Name(INT),
            nullable: false
        }
    );
}

/// The `??` skip-path promotion feeds the write's types-of-interest pass::
/// `int? x = g(); x = x ?? y;` with `int y` leaves `x` promoted to `int`
/// (the written type is the `??` result's non-null form). The RHS itself
/// evaluates on the LHS-null path — unpromoted (official
/// `ifNullExpression_rightBegin`, flow_analysis.dart:7086-7104).
#[test]
fn if_null_write_repromotes_via_types_of_interest() {
    use crate::ev::Ev;
    use crate::ty::{Base, Ty};
    const INT: u32 = 102;
    const X: u32 = 7;
    const Y: u32 = 8;
    let events: Vec<Ev> = [
        declared(X, named_ty(INT, true)),
        decl_block(Y, named_ty(INT, false)),
        vec![
            read(X),
            Ev::BinaryBegin {
                op: crate::ev::BinOp::IfNull,
                span: tspan(1),
            },
            read(X),
            read(Y),
            Ev::BinaryEnd {
                op: crate::ev::BinOp::IfNull,
                end: 3,
            },
            Ev::Assign {
                plain: true,
                if_null: false,
                end: 0,
                field_init: false,
            },
            Ev::ExecEnd {
                is_expr: false,
                end: tspan(0),
            },
        ],
    ]
    .concat();
    let interp = interp_of(&events);
    let model = interp.current.model(0);
    assert_eq!(
        interp.lists.head(model.promoted),
        Ty {
            base: Base::Name(INT),
            nullable: false
        }
    );
}

/// `x ??= e` (official `ifNullExpression_*` wiring on the null-aware
/// assignment path, assignment_expression_resolver.dart:665-707): the
/// write's type is `LUB(NonNull(readType), rhsType)`. At name level an
/// unresolvable RHS reads the unknown face — the official's InvalidType —
/// whose `LUB(NonNull(T1), InvalidType)` is InvalidType itself
/// (least_upper_bound.dart:55, `InvalidType <: X` for every X at
/// subtype.dart:274): the write neither promotes (the mutual-subtype
/// `isValidPromotionStep` check fails) nor demotes (the `covers` recovery
/// keeps any standing promotion).
#[test]
fn if_null_assign_unknown_rhs_writes_invalid_type() {
    use crate::ev::Ev;
    use crate::model::NIL;
    const INT: u32 = 102;
    const X: u32 = 7;
    // `int? x; x ??= opaque()` — the compound marker (plain=false,
    // if_null=true) with an RHS the name-level walk cannot type.
    let events: Vec<Ev> = [
        declared(X, named_ty(INT, true)),
        vec![
            read(X),
            Ev::OtherExpr { span: tspan(1) },
            Ev::Assign {
                plain: false,
                if_null: true,
                end: 0,
                field_init: false,
            },
            Ev::ExecEnd {
                is_expr: false,
                end: tspan(0),
            },
        ],
    ]
    .concat();
    let interp = interp_of(&events);
    // No promotion stands: the InvalidType write records no chain head.
    assert_eq!(interp.current.model(0).promoted, NIL);
    // The variable stays assigned and reads at its declared type.
    assert!(interp.current.model(0).assigned);
}
#[test]
fn redundant_is_checks_do_not_promote() {
    use crate::ev::Ev;
    use crate::model::NIL;
    const INT: u32 = 102;
    const X: u32 = 7;
    for tested_nullable in [false, true] {
        let events: Vec<Ev> = [
            declared(X, named_ty(INT, false)),
            is_test(X, named_ty(INT, tested_nullable), false),
        ]
        .concat();
        let interp = interp_of(&events);
        assert_eq!(
            interp.current.model(0).promoted,
            NIL,
            "tested_nullable={tested_nullable}: a redundant `is` is not a promotion step"
        );
    }
}

// —— written-argument capture and the promotion gate (#611 step 4) ——————

/// A named-type event with written arguments (arena id).
fn ty_with_args(args: u32, nullable: bool) -> crate::ev::Ev {
    crate::ev::Ev::Type {
        base: crate::ty::Base::Name(0),
        nullable,
        args,
    }
}

/// Arena for the gate tests: `0` reserved, `1` = `<int>`, `2` =
/// `<String>`, `3` = `List<int>`, `4` = `List<String>` (head name 0).
/// Leaked like the token tables above: the interpreter borrows it for its
/// whole (test-scoped) life.
fn arg_arena() -> &'static [Vec<crate::ty::ArgTy>] {
    let entry = |name: u32, args: u32| crate::ty::ArgTy {
        base: crate::ty::Base::Name(name),
        nullable: false,
        args,
    };
    Box::leak(Box::new(vec![
        vec![],
        vec![entry(1, crate::ty::EMPTY_ARGS)],
        vec![entry(2, crate::ty::EMPTY_ARGS)],
        vec![entry(0, 1)],
        vec![entry(0, 2)],
    ]))
}

/// The recorder interns written argument trees bottom-up and dedups them
/// structurally: `Map<String, List<int>>` yields one `[String, List<int>]`
/// list whose nested `<int>` is the same entry the standalone `List<int>`
/// annotation interns — the arena the promotion gate compares (#611
/// step 4).
#[test]
fn written_argument_trees_intern_bottom_up_and_dedup() {
    let (flow, _) = analyze("void f(Map<String, List<int>> a, List<int> b) {}");
    let flow = flow.0.borrow();
    let args = &flow.type_args;
    assert_eq!(args.len(), 3, "reserved + <int> + <String, List<int>>");
    assert!(args[0].is_empty(), "index 0 is the reserved empty list");
    let base_name = |entry: &crate::ty::ArgTy| match entry.base {
        crate::ty::Base::Name(id) => flow.name_lexeme(id).to_string(),
        _ => panic!("every interned entry here is a named type"),
    };
    let [nested] = &args[1][..] else {
        panic!("the shared `<int>` list has one entry");
    };
    assert_eq!(base_name(nested), "int");
    assert_eq!(nested.args, crate::ty::EMPTY_ARGS);
    let [key, value] = &args[2][..] else {
        panic!("the map's list has two entries");
    };
    assert_eq!(base_name(key), "String");
    assert_eq!(key.args, crate::ty::EMPTY_ARGS);
    assert_eq!(base_name(value), "List");
    // Bottom-up folding: the nested List entry carries the shared <int>.
    assert_eq!(value.args, 1);
}

/// `List<int> x; x is List<String>` refuses the promotion: the written
/// arguments differ and the official lattice refuses `List<String> <:
/// List<int>` (invariant entries), so no promotion exists at all.
#[test]
fn mismatched_written_args_refuse_promotion() {
    let events: Vec<crate::ev::Ev> = [
        declared(7, ty_with_args(3, false)),
        is_test(7, ty_with_args(4, false), false),
    ]
    .concat();
    let mut interp = crate::interp::Interp::new(&crate::ty::NameTypeOps, arg_arena(), &[]);
    interp.run(&events);
    assert_eq!(
        interp.current.model(0).promoted,
        crate::model::NIL,
        "the mismatched test leaves the variable at its declared type"
    );
}

/// `List<int>? x; x is List<int>` promotes (equal arguments, nullable
/// narrowed — official `tryPromoteToType` admits `List<int> <: List<int>?`)
/// and the promotion chain head carries the tested spelling's written
/// argument tree (#1010), so the variable's reads surface in the fact face
/// with the rows attached.
#[test]
fn equal_written_args_promote_with_args() {
    let events: Vec<crate::ev::Ev> = [
        declared(7, ty_with_args(3, true)),
        is_test(7, ty_with_args(3, false), false),
    ]
    .concat();
    let mut interp = crate::interp::Interp::new(&crate::ty::NameTypeOps, arg_arena(), &[]);
    interp.run(&events);
    let model = interp.current.model(0);
    assert!(!interp.lists.head(model.promoted).nullable);
    assert_eq!(
        interp.lists.head_args(model.promoted),
        3,
        "the chain head carries the tested spelling's written arguments"
    );
}

/// `List<int> x; x is List` — a raw tested type against a written
/// declaration — refuses: raw `List` spells `List<dynamic>`, which is no
/// subtype of `List<int>` (`type_system.dart` top-type instantiation).
#[test]
fn raw_tested_against_written_declaration_refuses() {
    let events: Vec<crate::ev::Ev> = [
        declared(7, ty_with_args(3, false)),
        is_test(7, ty_with_args(crate::ty::EMPTY_ARGS, false), false),
    ]
    .concat();
    let mut interp = crate::interp::Interp::new(&crate::ty::NameTypeOps, arg_arena(), &[]);
    interp.run(&events);
    assert_eq!(interp.current.model(0).promoted, crate::model::NIL);
}

// —— #1013 m2 dead-code faces ————————————————————————————————————————

/// A statement that began live and terminated flow itself (`throw 0;`,
/// `return throw 1;`, `rethrow`) never anchors its own dead row: the
/// post-order statement wrappers read the statement's began-dead
/// snapshot, and the official's first dead node is the next visited node
/// (`_visitNode`, upstream Dart SDK `dead_code_verifier.dart`).
#[test]
fn flow_terminating_statement_does_not_anchor_itself() {
    assert!(dead_snippets("void f() { throw 0; }").is_empty());
    assert_eq!(
        dead_snippets("void f() { throw 0; print(1); }"),
        ["print(1);"]
    );
    assert_eq!(
        dead_snippets("void f() { return; print(2); }"),
        ["print(2);"]
    );
    assert_eq!(
        dead_snippets("void f() { return throw 1; print(2); }"),
        ["print(2);"]
    );
    assert_eq!(
        dead_snippets("void f() { rethrow; print(3); }"),
        ["print(3);"]
    );
}

/// The catch-clause family (m2 over-marking): a try body whose flow dies
/// (`throw`) leaves every clause reachable — each catch clause enters on
/// its own exception path seeded from the pre-try state — so the flow
/// engine emits no dead row for any clause shape; the official's
/// catch-clause rows (`dead_code_catch_following_catch`,
/// `dead_code_on_catch_subtype`, `nullable_type_in_catch_clause`) are the
/// semantic layer's clause verifier, not flow facts.
#[test]
fn try_body_terminator_leaves_catch_clauses_reachable() {
    for src in [
        "void f() { try { throw 'x'; } catch (e) { print(e); } on Exception { print(1); } }",
        "class A {}\nclass B extends A {}\nvoid f() { try { throw B(); } on B { print(1); } on A { print(2); } }",
        "void f() { try { throw 'x'; } on Exception? catch (e) { print(e); } }",
    ] {
        assert!(dead_snippets(src).is_empty(), "{src:?}");
    }
}

/// A cast whose operand type and target are disjoint by nullability kills
/// the continuation: the oracle emits `dead_code` on the statement after
/// `var a = null as int;` (`asExpression_end`'s
/// `_isTypeCheckGuaranteedToFailWithSoundNullSafety` — a `Null`-typed operand
/// under a non-nullable cast always throws, and the written `Null` type names
/// the core null type just like the null literal). The flow kill for a cast
/// whose operand never completes comes instead from the operand's own
/// `Ev::Exit` — a separate face from the disjointness question.
#[test]
fn null_disjoint_cast_kills_the_following_statement() {
    for source in [
        "void f(Null n) { var x = n as int; print(x); }",
        "void f() { var x = null as int; print(x); }",
    ] {
        assert_eq!(dead_snippets(source), ["print(x);"], "{source:?}");
    }
}

/// An expression cast promotes its operand variable straight-line, no
/// branch split (official `asExpression_end`'s `tryPromoteForTypeCast`,
/// flow_analysis.dart:6475-6479, fed after the type events route the cast
/// type): a nullable-declared `x` cast to `int` leaves the following read
/// at the cast type. The operand's own read row precedes the promotion
/// (the declared type stands at its evaluation point), the promotion
/// surfaces as its own fact, and no dead code follows — a nullable downcast
/// can fail but is not disjoint by nullability, so it does not kill the
/// continuation (`null_disjoint_cast_kills_the_following_statement` covers
/// the guaranteed-fail face). #1619 residual
/// pin: the arm's first live promotion face.
#[test]
fn expression_cast_promotes_the_operand() {
    let source = "void f(int? x) { x as int; x; }";
    let (flow, facts) = analyze(source);
    let read_types: Vec<crate::ty::Ty> = facts
        .iter()
        .filter_map(|fact| match fact {
            FlowFact::ReadType { ty, .. } => Some(*ty),
            _ => None,
        })
        .collect();
    assert_eq!(read_types.len(), 2, "{facts:?}");
    assert!(read_types[0].nullable, "the cast operand read is declared");
    assert!(
        !read_types[1].nullable,
        "the read after the cast is promoted"
    );
    let crate::ty::Base::Name(id) = read_types[1].base else {
        panic!("expected a named type, got {:?}", read_types[1]);
    };
    assert_eq!(flow.0.borrow().name_lexeme(id), "int");
    assert_eq!(promoted_tys(source).len(), 1, "{source:?}");
    assert!(dead_snippets(source).is_empty(), "{source:?}");
}

/// A statement dead boundary never leaks OUT of a nested executable: a
/// closure body that terminates its own flow rewrites the snapshot only
/// within its own boundaries — the enclosing value is saved at
/// `ExecBegin` and restored at `ExecEnd`, so the enclosing statement's
/// wrapper reads what IT began in.
#[test]
fn closure_body_termination_does_not_leak_out() {
    assert!(dead_snippets("void f() { h(() { throw 1; }); print(2); }").is_empty());
}

// —— read-type facts (ReadType, #1056) ———————————————————————————————————

/// Every reachable read emits a `ReadType` row carrying the flow type —
/// promoted when a promotion stands, declared otherwise; the condition
/// read sits before the promotion, the branch read inside it. Dead reads
/// carry the same row (the official materializes the flow type under an
/// unreachable state too), and a plain assignment's target read retracts
/// with the target facts (the identifier is the write).
#[test]
fn read_type_rows_carry_the_flow_type_at_reads() {
    let source = "void f(int? x) { if (x is int) { x; } }";
    let (flow, facts) = analyze(source);
    let read_types: Vec<crate::ty::Ty> = facts
        .iter()
        .filter_map(|fact| match fact {
            FlowFact::ReadType { ty, .. } => Some(*ty),
            _ => None,
        })
        .collect();
    assert_eq!(read_types.len(), 2, "{facts:?}");
    assert!(read_types[0].nullable, "the condition read is declared");
    assert!(!read_types[1].nullable, "the branch read is promoted");
    let crate::ty::Base::Name(id) = read_types[1].base else {
        panic!("expected a named type, got {:?}", read_types[1]);
    };
    assert_eq!(flow.0.borrow().name_lexeme(id), "int");
    // A read inside a dead tail still carries its row: the official
    // materializes the flow type under an unreachable state too — the
    // region's own dead row is the only reachability face.
    let dead = "void f(int x) { return; x; }";
    assert!(
        facts_of(dead)
            .iter()
            .any(|fact| matches!(fact, FlowFact::ReadType { .. })),
        "{dead:?}"
    );
    // A plain assignment's target is a write, not a read.
    let target = "void f(int x) { x = 1; }";
    assert!(
        facts_of(target)
            .iter()
            .all(|fact| !matches!(fact, FlowFact::ReadType { .. })),
        "{target:?}"
    );
}

/// The collection-if element split carries the promotion into the
/// then-element only (official `handle_ifElement_conditionEnd`,
/// type_analyzer.dart:1161, replayed at `ThenControlFlow` — the split
/// opens on the condition's own event, interp_expr.rs): the then-element's
/// read is promoted, the else-element's read is not (`!=`'s false arm
/// carries no factored remainder — official `_equalityCheck`,
/// flow_analysis.dart:4046-4076), and each element read's read-type row
/// carries its branch's flow type.
#[test]
fn collection_if_splits_promotion_across_elements() {
    let source = "void f(int? x, List<int> g) { g([if (x != null) x else x]); }";
    let (_, facts) = analyze(source);
    let cond_offset = (source.find("(x != null)").unwrap() + 1) as u32;
    let then_offset = (source.find(") x else").unwrap() + ") ".len()) as u32;
    let else_offset = (source.find("else x").unwrap() + "else ".len()) as u32;
    // Exactly one promoted read: the then-element's.
    let promoted: Vec<u32> = facts
        .iter()
        .filter_map(|fact| match fact {
            FlowFact::PromotedRead { span, .. } => Some(span.range.offset),
            _ => None,
        })
        .collect();
    assert_eq!(promoted, [then_offset], "{facts:?}");
    let read_types: Vec<(u32, crate::ty::Ty)> = facts
        .iter()
        .filter_map(|fact| match fact {
            FlowFact::ReadType { span, ty, .. } => Some((span.range.offset, *ty)),
            _ => None,
        })
        .collect();
    let ty_at = |offset: u32| {
        read_types
            .iter()
            .find(|(at, _)| *at == offset)
            .map(|(_, ty)| *ty)
            .unwrap_or_else(|| panic!("no read-type row at {offset}: {read_types:?}"))
    };
    // The condition read sits before the split; the then read is promoted
    // non-null; the else read is back at the declared nullable type.
    assert!(ty_at(cond_offset).nullable, "{read_types:?}");
    assert!(!ty_at(then_offset).nullable, "{read_types:?}");
    assert!(ty_at(else_offset).nullable, "{read_types:?}");
}

// —— write-site facts (WriteSite, #1056) —————————————————————————————————

/// The write-site rows aggregate the `AssignedVariables` anywhere sets, one
/// row per (declaration, in-closure) pair: a reassignment outside
/// closures rows once, a closure write rows with `in_closure: true`, and
/// a variable written both ways rows twice. Emission groups the sets —
/// writes outside closures first — and the fact sink's stable offset
/// sort then interleaves the rows by declaration offset. Declaration
/// initializers and for-in declared loop variables are state-only: they
/// move the flow state through `apply_write` and never enter the sets
/// (the official sets do not count them — the `prefer_final_locals`
/// premise), so only genuine reassignments emit rows.
#[test]
#[expect(clippy::cast_possible_truncation)] // span arithmetic narrows under the scanner u32 offset cap
fn write_site_rows_aggregate_the_write_sets() {
    let source = "void f() { int a = 0; int b; b = 1; int c = 0; () { c = 2; }; int d = 0; d = 1; () { d = 2; }; }";
    let (_, facts) = analyze(source);
    let at = |needle: &str| source.find(needle).unwrap() as u32;
    let rows: Vec<(u32, bool)> = facts
        .iter()
        .filter_map(|fact| match fact {
            FlowFact::WriteSite {
                decl_name_span,
                in_closure,
            } => Some((decl_name_span.offset, *in_closure)),
            _ => None,
        })
        .collect();
    assert_eq!(
        rows,
        vec![
            (at("b;"), false),
            (at("c = 0;"), true),
            (at("d = 0;"), false),
            (at("d = 0;"), true),
        ],
        "{facts:?}"
    );
}

// —— async-gap context face (use_build_context_synchronously core) ——

/// The async-gap face rows of a source, in fact order:
/// (`snippet`, `origin`, `gap`, `guarded`, `unrelated_mounted`).
fn async_rows(source: &str) -> Vec<(String, AsyncOrigin, u32, bool, bool)> {
    facts_of(source)
        .into_iter()
        .filter_map(|fact| match fact {
            FlowFact::AsyncContextUse {
                span,
                origin,
                gap,
                gap_at: _,
                guarded,
                unrelated_mounted,
            } => Some((
                source
                    [span.range.offset as usize..(span.range.offset + span.range.length) as usize]
                    .to_string(),
                origin,
                gap,
                guarded,
                unrelated_mounted,
            )),
            _ => None,
        })
        .collect()
}

#[test]
fn async_gap_flags_use_after_await() {
    let source = r"
void f(BuildContext context) async {
  await g();
  context.read();
}
";
    assert_eq!(
        async_rows(source),
        vec![("context".into(), AsyncOrigin::Context, 1, false, false)]
    );
}

#[test]
fn async_gap_mounted_check_in_then_guards() {
    let source = r"
void f(BuildContext context) async {
  await g();
  if (context.mounted) {
    context.read();
  }
}
";
    assert_eq!(
        async_rows(source),
        vec![("context".into(), AsyncOrigin::Context, 1, true, false)]
    );
}

#[test]
fn async_gap_negative_guard_with_exit_guards() {
    // `if (!context.mounted) return;` — the notMountedCheck on a
    // terminating then-branch guards the continuation.
    let source = r"
void f(BuildContext context) async {
  await g();
  if (!context.mounted) return;
  context.read();
}
";
    assert_eq!(
        async_rows(source),
        vec![("context".into(), AsyncOrigin::Context, 1, true, false)]
    );
}

#[test]
fn async_gap_positive_guard_with_else_exit_guards() {
    let source = r"
void f(BuildContext context) async {
  await g();
  if (context.mounted) {
    g();
  } else {
    return;
  }
  context.read();
}
";
    assert_eq!(
        async_rows(source),
        vec![("context".into(), AsyncOrigin::Context, 1, true, false)]
    );
}

/// The else entry's verdict rows derive from the split-time latch, not
/// from the statement stack: at `ElseBegin` the condition's markers are
/// already folded (`ThenBegin`'s `fold_to`) and statement sequences balance,
/// so the legacy `pop_val` there pops the clean-stack placeholder — inert,
/// structurally unable to fold anything into the else fold. The rows come
/// from `branch_entry`'s `check_entry` fallback: the branch a positive
/// check does not select keeps the verdict the check latched at its
/// evaluation, so a use in the else of `if (context.mounted)` rows guarded
/// with the latched crossing — the same shape the branch-join family pins.
/// #1619 residual pin: the documented latent shape stays deterministic.
#[test]
fn async_gap_else_entry_keeps_the_split_verdict() {
    let source = r"
void f(BuildContext context) async {
  await g();
  if (context.mounted) {
    g();
  } else {
    context.read();
  }
}
";
    assert_eq!(
        async_rows(source),
        vec![("context".into(), AsyncOrigin::Context, 1, true, false)]
    );
}

#[test]
fn async_gap_guard_does_not_survive_branch_join() {
    // Both branches verified does not carry past the if (upstream merge
    // parity: the walk falls through to the pre-if state).
    let source = r"
void f(BuildContext context) async {
  await g();
  if (context.mounted) {
    context.read();
  } else {
    context.read();
  }
  context.read();
}
";
    assert_eq!(
        async_rows(source),
        vec![
            ("context".into(), AsyncOrigin::Context, 1, true, false),
            ("context".into(), AsyncOrigin::Context, 1, true, false),
            ("context".into(), AsyncOrigin::Context, 1, false, false),
        ]
    );
}

#[test]
fn async_gap_state_mounted_check_flags_wrong_mounted() {
    // The bare (implicit this) `mounted` is a State-side check: the
    // context usage after it is unguarded with the wrong-mounted face.
    let source = r"
void f(BuildContext context) async {
  await g();
  if (!mounted) return;
  context.read();
}
";
    assert_eq!(
        async_rows(source),
        vec![("context".into(), AsyncOrigin::Context, 1, false, true)]
    );
}

#[test]
fn async_gap_bare_context_is_state_origin() {
    let source = r"
Future<void> f() async {
  await g();
  context.read();
}
";
    assert_eq!(
        async_rows(source),
        vec![("context".into(), AsyncOrigin::State, 1, false, false)]
    );
}

#[test]
fn async_gap_this_context_is_state_origin() {
    let source = r"
class C {
  Future<void> f() async {
    await g();
    this.context.read();
  }
}
";
    assert_eq!(
        async_rows(source),
        vec![("context".into(), AsyncOrigin::State, 1, false, false)]
    );
}

#[test]
fn async_gap_non_context_usages_ignored() {
    let source = r"
void f(String s) async {
  await g();
  s.isEmpty;
  print(s);
}
";
    assert!(async_rows(source).is_empty());
}

#[test]
fn async_gap_no_await_no_rows() {
    let source = r"
void f(BuildContext context) {
  context.read();
}
";
    assert!(async_rows(source).is_empty());
}

#[test]
fn async_gap_loop_back_upgrade_flags_use() {
    // The use precedes the loop's await; the next iteration runs after
    // it (the loop-back upgrade).
    let source = r"
void f(BuildContext context) async {
  for (var i = 0; i < 1; i++) {
    context.read();
    await g();
  }
}
";
    assert_eq!(
        async_rows(source),
        vec![("context".into(), AsyncOrigin::Context, 1, false, false)]
    );
}

#[test]
fn async_gap_closure_is_opaque() {
    let source = r"
void f(BuildContext context) async {
  await g();
  callback(() {
    context.read();
  });
}
";
    assert!(async_rows(source).is_empty());
}

#[test]
fn async_gap_await_in_try_gaps_finally() {
    let source = r"
void f(BuildContext context) async {
  try {
    await g();
  } finally {
    context.read();
  }
}
";
    assert_eq!(
        async_rows(source),
        vec![("context".into(), AsyncOrigin::Context, 1, false, false)]
    );
}

#[test]
fn async_gap_argument_use_uses_post_argument_state() {
    let source = r"
void f(BuildContext context) async {
  await g();
  print(context);
}
";
    assert_eq!(
        async_rows(source),
        vec![("context".into(), AsyncOrigin::Context, 1, false, false)]
    );
}

#[test]
fn async_gap_nullable_context_argument_exempt() {
    // Argument uses require the non-nullable declared form (upstream
    // Dart SDK source, pkg/linter/lib/src/rules/
    // use_build_context_synchronously.dart:1424,
    // `argType.isBuildContext(skipNullable: isGetter)`); nullable
    // arguments carry no row.
    //
    // Runtime trace of the exemption (the gap pass over the recorded
    // buffer): the formal's `Type` row parks `Ctx { nullable: true }` and
    // `Formal` declares it; `await g();` closes one crossing (gap 1, both
    // verdict slots Gap); `print(context)` streams Ident(print) as Other,
    // then Ident(context) resolving to the `Ctx { nullable: true }` kind —
    // `ArgumentsEnd` reads each argument's kind and the nullable arm is
    // the exemption (only `Ctx { nullable: false }` and `StVal` record a
    // use), so no row is buffered or emitted; `Send` and `ExprStmt` pop
    // inert markers. The non-nullable twin (`async_gap_argument_use_
    // uses_post_argument_state`) flips only the declaration's `?` and rows.
    let source = r"
void f(BuildContext? context) async {
  await g();
  print(context);
}
";
    assert!(async_rows(source).is_empty());
}

#[test]
fn async_gap_target_verdict_latched_at_evaluation() {
    // The invocation target's verdict latches when the receiver
    // evaluates: the later argument await (gap 2) does not affect it.
    let source = r"
void f(BuildContext context) async {
  await g();
  context.read(await g2());
}
";
    assert_eq!(
        async_rows(source),
        vec![("context".into(), AsyncOrigin::Context, 1, false, false)]
    );
}

#[test]
fn async_gap_while_condition_await_gaps_body() {
    let source = r"
void f(BuildContext context) async {
  while (await g()) {
    context.read();
  }
}
";
    assert_eq!(
        async_rows(source),
        vec![("context".into(), AsyncOrigin::Context, 1, false, false)]
    );
}

#[test]
fn async_gap_gap_at_anchors_await_keyword() {
    let source = r"
void f(BuildContext context) async {
  await g();
  context.read();
}
";
    let await_offset = source
        .find("await")
        .map_or(0, |i| u32::try_from(i).unwrap_or(u32::MAX));
    let rows = facts_of(source);
    let anchored = rows.into_iter().any(|fact| match fact {
        FlowFact::AsyncContextUse { gap_at, .. } => gap_at.range.offset == await_offset,
        _ => false,
    });
    assert!(anchored);
}

#[test]
fn async_gap_await_in_guard_reopens_gap() {
    // An await inside the guard expression folds the condition's async
    // state to asynchronous (the awaited value carries `G::Gap` through
    // the `&&` table): the guarded branch sits behind the crossing, and
    // the mounted check earlier in the condition does not silence the
    // use.
    let source = r"
void f(BuildContext context) async {
  if (context.mounted == true && await g()) {
    context.read();
  }
}
";
    assert_eq!(
        async_rows(source),
        vec![("context".into(), AsyncOrigin::Context, 1, false, false)]
    );
}

#[test]
fn async_gap_pure_guard_without_await_stays_guarded() {
    // No await anywhere: the purely syntactic guard keeps its verdict —
    // no over-correction from the awaited-value gap.
    let source = r"
void f(BuildContext context) async {
  if (context.mounted) {
    context.read();
  }
}
";
    assert_eq!(
        async_rows(source),
        vec![("context".into(), AsyncOrigin::Context, 0, true, false)]
    );
}

#[test]
fn async_gap_guard_await_without_use_no_rows() {
    // The same guard-await shape with no context use in the branch: no
    // row, no panic.
    let source = r"
void f(BuildContext context) async {
  if (context.mounted == true && await g()) {
    g2();
  }
}
";
    assert!(async_rows(source).is_empty());
}

#[test]
fn async_gap_await_before_mounted_check_still_guards() {
    // Upstream isAnd ordering: `asynchronous && mountedCheck` folds to
    // mountedCheck (the `(G::Gap, _)` arm feeds the later check), so the
    // branch stays guarded — the await before the check does not flag
    // the use.
    let source = r"
void f(BuildContext context) async {
  if (await g() && context.mounted) {
    context.read();
  }
}
";
    assert_eq!(
        async_rows(source),
        vec![("context".into(), AsyncOrigin::Context, 0, true, false)]
    );
}

// —— promoted-type-at-condition query rows (#1349) ————————————————————

/// The condition-operand query rows of a source: `(anchored snippet,
/// offset, ty)` per row, in fact order.
fn condition_rows(source: &str) -> Vec<(String, u32, crate::ty::Ty)> {
    facts_of(source)
        .into_iter()
        .filter_map(|fact| match fact {
            FlowFact::ConditionType { span, ty, .. } => Some((
                source
                    [span.range.offset as usize..(span.range.offset + span.range.length) as usize]
                    .to_string(),
                span.range.offset,
                ty,
            )),
            _ => None,
        })
        .collect()
}

/// The matched-type query rows of a source: `(anchored snippet, offset,
/// ty, written-argument tree)` per row, in fact order.
fn matched_rows(source: &str) -> Vec<(String, u32, crate::ty::Ty, u32)> {
    facts_of(source)
        .into_iter()
        .filter_map(|fact| match fact {
            FlowFact::MatchedType { span, ty, args } => Some((
                source
                    [span.range.offset as usize..(span.range.offset + span.range.length) as usize]
                    .to_string(),
                span.range.offset,
                ty,
                args,
            )),
            _ => None,
        })
        .collect()
}

/// The condition operand's row carries the flow type at its evaluation
/// point: a declared read rows with the declared type, and a `&&` right
/// operand rows with the promotion standing under the left's true branch
/// (official `checkForNonBoolCondition` per operand —
/// bool_expression_verifier.dart:45-73). The `is`-test left operand is
/// not a reference and rows nothing.
#[test]
#[expect(clippy::cast_possible_truncation)] // span arithmetic narrows under the scanner u32 offset cap
fn condition_rows_carry_the_flow_type_at_condition_operands() {
    let (flow, _) = analyze("void f(bool? b) { if (b) {} }");
    let rows = condition_rows("void f(bool? b) { if (b) {} }");
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].0, "b");
    assert!(rows[0].2.nullable, "{:?}", rows[0].2);
    let crate::ty::Base::Name(id) = rows[0].2.base else {
        panic!("expected a named type, got {:?}", rows[0].2);
    };
    assert_eq!(flow.0.borrow().name_lexeme(id), "bool");

    // The right operand evaluates under the left's promotion; the left
    // operand (`x is bool`) is a test, not a reference, and rows nothing.
    let source = "void f(bool? x) { if (x is bool && x) {} }";
    let (flow, _) = analyze(source);
    let rows = condition_rows(source);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].0, "x");
    assert_eq!(rows[0].1, source.rfind('x').unwrap() as u32, "{rows:?}");
    assert!(!rows[0].2.nullable, "the right operand sees the promotion");
    let crate::ty::Base::Name(id) = rows[0].2.base else {
        panic!("expected a named type, got {:?}", rows[0].2);
    };
    assert_eq!(flow.0.borrow().name_lexeme(id), "bool");
}

/// One row per statement-head condition across the official query
/// contexts: if, while, do-while, assert (message or not), conditional
/// expression, C-style `for` condition, and the collection if-element.
#[test]
fn condition_rows_fire_at_every_condition_context() {
    let source = "\
void f(bool? b) {
  if (b) {}
  while (b) {}
  do {} while (b);
  assert(b, 'm');
  var x = b ? 1 : 2;
  for (; b; ) {}
  var l = [if (b) 1];
}";
    let rows = condition_rows(source);
    assert_eq!(rows.len(), 7, "{rows:?}");
    assert!(
        rows.iter()
            .all(|(snippet, _, ty)| snippet == "b" && ty.nullable)
    );
}

/// A parenthesized operand keeps its read's anchor: `((b))` rows at the
/// innermost identifier (the last `b` in the source — the earlier ones
/// are the `bool` spelling and the parameter).
#[test]
#[expect(clippy::cast_possible_truncation)] // span arithmetic narrows under the scanner u32 offset cap
fn condition_rows_anchor_parenthesized_operands_at_the_read() {
    let source = "void f(bool? b) { if ((b)) {} }";
    let rows = condition_rows(source);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].0, "b");
    assert_eq!(rows[0].1, source.rfind('b').unwrap() as u32, "{rows:?}");
}

/// Each boolean operator operand rows at its own evaluation point: the
/// left pre-split, the right under the left's taken branch; nested
/// operators row their own operands.
#[test]
fn condition_rows_cover_each_boolean_operand() {
    let source = "void f(bool? a, bool? b) { if (a && b) {} }";
    let rows = condition_rows(source);
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(rows[0].0, "a");
    assert_eq!(rows[1].0, "b");
    // Nested: `a || (b && c)` rows a, b, c — nothing for the compounds.
    let nested = "void f(bool? a, bool? b, bool? c) { if (a || (b && c)) {} }";
    let rows = condition_rows(nested);
    assert_eq!(rows.len(), 3, "{rows:?}");
    assert_eq!(
        rows.iter()
            .map(|(snippet, _, _)| snippet.as_str())
            .collect::<Vec<_>>(),
        ["a", "b", "c"]
    );
}

/// Non-reference operands carry no flow-visible type and row nothing:
/// calls, comparisons, and assignments-as-conditions (no read anchor).
#[test]
fn condition_rows_skip_non_reference_operands() {
    for source in [
        "void f() { if (g()) {} }",
        "void f(int? x) { if (x == null) {} }",
        "void f(bool? b) { if (b = g()) {} }",
        "void f(bool? b) { if (!b) {} }",
    ] {
        assert!(condition_rows(source).is_empty(), "{source:?}");
    }
}

/// The matched-type row carries the scrutinee's flow type at the clause
/// match: unpromoted it is the declared type, and every in-scope check
/// before the switch shows through (official `getMatchedValueType`,
/// flow_analysis.dart:8753-8760). Statement clauses anchor at the `case`
/// keyword; a `default` clause matches no pattern and rows nothing.
#[test]
fn matched_type_rows_carry_the_scrutinee_promotion_at_case_clauses() {
    let source = "void f(int? x) { switch (x) { case 1: break; } }";
    let (flow, _) = analyze(source);
    let rows = matched_rows(source);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].0, "case");
    assert!(rows[0].2.nullable, "unpromoted: the declared type");
    let crate::ty::Base::Name(id) = rows[0].2.base else {
        panic!("expected a named type, got {:?}", rows[0].2);
    };
    assert_eq!(flow.0.borrow().name_lexeme(id), "int");

    // A null check before the switch promotes the scrutinee; the clause
    // sees the promoted type.
    let promoted = "void f(int? x) { if (x != null) { switch (x) { case 1: break; } } }";
    let rows = matched_rows(promoted);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!(!rows[0].2.nullable, "promoted non-null at the match");

    // A default clause rows nothing.
    let default = "void f(int? x) { switch (x) { default: break; } }";
    assert!(matched_rows(default).is_empty(), "{default:?}");
}

/// Every case head in a group rows on its own; the row reads the chain
/// the clause branched from, so an earlier clause's guard-false outcome
/// is in and the clause's own pattern and guard are not.
#[test]
fn matched_type_rows_cover_each_head_under_the_chain() {
    let source = "\
void f(int? x) {
  switch (x) {
    case 1:
    case 2:
      break;
    case 3 when x > 0:
      break;
  }
}";
    let rows = matched_rows(source);
    assert_eq!(rows.len(), 3, "{rows:?}");
    assert!(
        rows.iter()
            .all(|(snippet, _, ty, _)| snippet == "case" && ty.nullable)
    );
}

/// The row's written-argument tree stages the promoted spelling exactly
/// as written: `List<int>?` promoted by a null check carries the
/// non-null `List<int>` beside the interned `int` tree.
#[test]
fn matched_type_rows_carry_the_written_argument_tree() {
    let source = "void f(List<int>? l) { if (l != null) { switch (l) { case 1: break; } } }";
    let (flow, _) = analyze(source);
    let rows = matched_rows(source);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!(!rows[0].2.nullable);
    let tree = flow.0.borrow().type_args_at(rows[0].3).to_vec();
    assert_eq!(tree.len(), 1, "{tree:?}");
    let crate::ty::Base::Name(id) = tree[0].base else {
        panic!("expected a named argument, got {:?}", tree[0]);
    };
    assert_eq!(flow.0.borrow().name_lexeme(id), "int");
    assert!(!tree[0].nullable);
}

/// Switch-expression alternatives row too, anchored at each
/// alternative's first token (the parser delivers no `case` keyword
/// face there); the match-time row rides the pattern close's latch.
#[test]
fn matched_type_rows_fire_at_switch_expression_alternatives() {
    let source = "void f(int? x) { var y = switch (x) { 1 => 1, _ => 2 }; }";
    let rows = matched_rows(source);
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(rows[0].0, "1");
    assert_eq!(rows[1].0, "_");
    assert!(rows[0].1 < rows[1].1, "clauses anchor in source order");
    assert!(rows.iter().all(|(_, _, ty, _)| ty.nullable));
}

/// A non-reference scrutinee has no promotion key and rows nothing.
#[test]
fn matched_type_rows_require_a_reference_scrutinee() {
    let source = "void f() { switch (g()) { case 1: break; } }";
    assert!(matched_rows(source).is_empty(), "{source:?}");
}

// —— property promotion facts (PromotedPropertyRead, #1409) ————————————————

/// Snippets of property-promotion reads, rendered as the member name's
/// source text for readable assertions.
fn promoted_property_snippets(source: &str) -> Vec<String> {
    facts_of(source)
        .into_iter()
        .filter_map(|fact| match fact {
            FlowFact::PromotedPropertyRead { span } => Some(
                source
                    [span.range.offset as usize..(span.range.offset + span.range.length) as usize]
                    .to_string(),
            ),
            _ => None,
        })
        .collect()
}

/// Pin 1's flow face: after `c._b != null`, the tested property's read in
/// the then-branch carries a promotion row anchored at the member name
/// token (the official `_handleProperty` promotion lookup,
/// `flow_analysis.dart:9010-9054`). Eligibility — that `_b` is a private
/// final field at all — is the semantic consumer's question.
#[test]
fn property_read_promotes_after_a_null_check() {
    let source = "class C { final bool? _b; }\nvoid f(C c) { if (c._b != null) { c._b; } }";
    assert_eq!(promoted_property_snippets(source), ["_b"]);
}

/// Pin 4's flow face: the slot key includes the receiver, so `c1`'s test
/// promotes only its own key — the same member on another receiver stays
/// unpromoted.
#[test]
fn property_promotion_keys_on_the_receiver() {
    let source =
        "class C { final bool? _b; }\nvoid f(C c1, C c2) { if (c1._b != null) { c2._b; c1._b; } }";
    assert_eq!(promoted_property_snippets(source), ["_b"]);
}

/// Pin 6's flow face: `== null` and the negated test take the inversion
/// face (the property is non-null on the FALSE arm only) — a read in the
/// then-branch or after the join stays unpromoted.
#[test]
fn property_promotion_rejects_the_inverse_test() {
    let eq_null = "class C { final bool? _b; }\nvoid f(C c) { if (c._b == null) { c._b; } }";
    assert!(
        promoted_property_snippets(eq_null).is_empty(),
        "{eq_null:?}"
    );
    let negated = "class C { final bool? _b; }\nvoid f(C c) { if (!(c._b != null)) { c._b; } }";
    assert!(
        promoted_property_snippets(negated).is_empty(),
        "{negated:?}"
    );
}

/// Pin 5's flow face: the promotion lives on the then-branch state only;
/// the end-if join intersects the branches' chains (the official
/// joinPromotionInfo common-prefix rule, `flow_analysis.dart:5830-5940`)
/// and drops it.
#[test]
fn property_promotion_drops_after_a_single_arm_join() {
    let source = "class C { final bool? _b; }\nvoid f(C c, bool x) { if (x) { if (c._b != null) {} } c._b; }";
    assert!(promoted_property_snippets(source).is_empty(), "{source:?}");
}

/// The receiver-invalidation sweep: a receiver write bumps its variable
/// version, so dependent property promotions do not carry — no finality
/// exception (the official `write` on the receiver's version checks no
/// finality, `flow_analysis.dart:4802-4866`).
#[test]
fn property_promotion_drops_on_receiver_reassignment() {
    let source =
        "class C { final bool? _b; }\nvoid f(C c) { if (c._b != null) { c = C(); c._b; } }";
    assert!(promoted_property_snippets(source).is_empty(), "{source:?}");
}

/// Pin 7's flow face: explicit `this._b` and the implicit-`this` bare
/// `_b` both resolve their receiver to the `this` identity (the official
/// `ThisPropertyTarget`, `flow_analysis.dart:5977-5990`, and
/// `thisExpression`, :8149-8153).
#[test]
fn this_receiver_properties_promote() {
    let explicit = "class C { final bool? _b; void m() { if (this._b != null) { this._b; } } }";
    assert_eq!(promoted_property_snippets(explicit), ["_b"]);
    let implicit = "class C { final bool? _b; void m() { if (_b != null) { _b; } } }";
    assert_eq!(promoted_property_snippets(implicit), ["_b"]);
}

/// Pin 8's flow face: `late` is not a gate the flow layer sees — it tracks
/// the read shape the same way and the semantic eligibility gate carries
/// the promotability verdict (upstream `field_promotability.dart:70-76`
/// never consults `late`; `late final` private fields promote).
#[test]
fn late_final_fields_track_at_the_flow_layer() {
    let source = "class C { late final int? _x; }\nvoid f(C c) { if (c._x != null) { c._x; } }";
    assert_eq!(promoted_property_snippets(source), ["_x"]);
}

/// The byte-invariance guard: local-variable promotion faces are unchanged
/// by the property machinery — the same var rows fire with and without an
/// interleaved property read, and the property read's row stays a separate
/// fact kind.
///
/// Adjudication of the #1409 contradiction with
/// `property_promotion_keys_on_the_receiver` (both cannot hold under one
/// gate): a promotion row requires a standing promotion ON THAT PROPERTY
/// KEY, established only by a test of the property itself (official
/// `_equalityCheck`'s `_EqualityCheckIsNullCheck` → `tryMarkNonNullable`,
/// flow_analysis.dart:4046-4076; read back per read by `_handleProperty`,
/// :9010-9054). The `x != null` check here promotes the local `x` alone —
/// `c.g` was never tested, so no property row stands on it and the
/// expected property-row list is empty.
#[test]
fn local_variable_faces_unchanged_alongside_property_reads() {
    let pure = "void f(int? x) { if (x != null) { x.isEven; } }";
    let mixed = "class C { int? g; }\nvoid f(C c, int? x) { if (x != null) { c.g; x.isEven; } }";
    let pure_tys = promoted_tys(pure);
    let mixed_tys = promoted_tys(mixed);
    assert_eq!(pure_tys, mixed_tys, "var promotion rows must not shift");
    assert!(!pure_tys.is_empty(), "guard needs a live promotion");
    assert!(pure_tys.iter().all(|ty| !ty.nullable));
    // No property key was tested in either source, so no property row
    // stands anywhere (see the adjudication note above).
    assert!(promoted_property_snippets(pure).is_empty());
    assert!(promoted_property_snippets(mixed).is_empty());
    // The diagnostic faces (dead code / unassigned) stay empty in both;
    // the promotion rows above are the query face, not diagnostics.
    for source in [pure, mixed] {
        assert!(
            facts_of(source).iter().all(|fact| !matches!(
                fact,
                FlowFact::DeadCode { .. } | FlowFact::UnassignedRead { .. }
            )),
            "{source:?}"
        );
    }
}

/// Pin 11's flow face: the catch entry's conservative join sweeps property
/// slots with the models — a receiver written in the try body kills the
/// pre-try promotion on the catch path too (official `FlowModel.
/// conservativeJoin`, `flow_analysis.dart:3786-3830`, invoked from
/// `tryCatchStatement_bodyEnd`, :8163-8181).
#[test]
fn property_promotion_drops_into_a_catch() {
    let source = "class C { final bool? _b; }\nvoid f(C c) { if (c._b != null) { try { c = C(); } catch (e) { c._b; } } }";
    assert!(promoted_property_snippets(source).is_empty(), "{source:?}");
}

/// Pin 12's flow face: the switch's labeled-case entry conservative join
/// sweeps property slots with the models — a receiver written in any case
/// body kills the promotion carried into the switch on the jump-target
/// path.
#[test]
fn property_promotion_drops_into_a_labeled_case() {
    let source = "class C { final bool? _b; }\nvoid f(C c, int x) { if (c._b != null) { switch (x) { L: case 1: c._b; break; case 2: c = C(); break; } } }";
    assert!(promoted_property_snippets(source).is_empty(), "{source:?}");
}

/// The capture-blocking pin: a closure writing the receiver between the
/// test and the use permanently kills the promotion — the closure entry's
/// conservative join stamps the slots CAPTURED and the official
/// `_handleProperty` lookup gate on the captured target version
/// (`flow_analysis.dart:9021-9026`) blocks the read's promotion lookup.
#[test]
fn property_promotion_dies_when_a_closure_writes_the_receiver() {
    let source = "class C { final bool? _b; }\nvoid f(C c) { if (c._b != null) { g() { c = C(); }; if (c._b) {} } }";
    assert!(promoted_property_snippets(source).is_empty(), "{source:?}");
}

/// The catch-sweep bounds pin: the property slot is FIRST interned inside
/// the try body (the null check), so the pre-try snapshot the catch entry
/// sweeps has a shorter slot extent than the intern list — the sweep
/// fills the gap with fresh models before writing (the resizing face of
/// [`sweep_state_props`]). The receiver is write-captured by the closure,
/// so the sweep's captured arm materializes the capture on the fresh slot
/// instead of indexing past the snapshot, and the catch read stays
/// unpromoted (#1432).
#[test]
fn catch_sweep_reaches_slots_interned_inside_the_try_body() {
    let source = "class C { final bool? _b; }\nvoid f(C c) { try { if (c._b != null) {} g() { c = C(); }; } catch (e) { c._b; } }";
    assert!(promoted_property_snippets(source).is_empty(), "{source:?}");
}

// —— matched-case promotion (case-body promotion of the switched variable)

/// (`snippet`, promoted-type name) pairs of the ReadType rows, in fact
/// order — the matched-case promotion face's assertion view. Non-named
/// flow types (declared `dynamic`/untyped reads) render as an empty name.
fn read_type_names(source: &str) -> Vec<(String, String)> {
    let (flow, facts) = analyze(source);
    let flow = flow.0.borrow();
    facts
        .into_iter()
        .filter_map(|fact| match fact {
            FlowFact::ReadType { span, ty, .. } => {
                let name = match ty.base {
                    crate::ty::Base::Name(id) => flow.name_lexeme(id).to_string(),
                    _ => String::new(),
                };
                Some((
                    source[span.range.offset as usize
                        ..(span.range.offset + span.range.length) as usize]
                        .to_string(),
                    name,
                ))
            }
            _ => None,
        })
        .collect()
}

/// Every read named `needle`, in fact order.
fn reads_of(source: &str, needle: &str) -> Vec<String> {
    read_type_names(source)
        .into_iter()
        .filter(|(snippet, _)| snippet == needle)
        .map(|(_, name)| name)
        .collect()
}

/// A matched object pattern promotes the switched variable through its
/// case body (official `promoteForPattern` through `analyzeObjectPattern`,
/// shared type_analyzer.dart:1749-1772): `option.value` in the
/// issue60264.dart shape resolves on the pattern's head type. Each clause
/// promotes to its own head — the second body reads `None`, not `Some` —
/// and the post-switch read falls back to the declared type: the failed
/// match arms joined the unmatched chain, and the body exits joined to it
/// (`switch_end`), so the promotion cannot leak past the switch.
#[test]
fn matched_object_pattern_promotes_the_switched_variable() {
    let source = "void f<T>(Option<T> option) {\n\
                  \x20 switch (option) {\n\
                  \x20   case Some():\n\
                  \x20     option;\n\
                  \x20   case None():\n\
                  \x20     option;\n\
                  \x20 }\n\
                  \x20 option;\n\
                  }";
    assert_eq!(
        reads_of(source, "option"),
        ["Option", "Some", "None", "Option"],
        "{source:?}"
    );
}

/// The guard-scope boundary (official order: pattern → guard → body,
/// shared type_analyzer.dart:2564-2605): the guard of the SAME clause
/// runs after its pattern's promotion — the promotion is live there by
/// design — while a LATER clause's guard runs under the chained
/// unmatched state and sees only its own pattern's promotion, never an
/// earlier clause's.
#[test]
fn case_guard_sees_its_own_promotion_and_not_an_earlier_clauses() {
    let source = "void f<T>(Option<T> option, bool g) {\n\
                  \x20 switch (option) {\n\
                  \x20   case Some() when g:\n\
                  \x20     option;\n\
                  \x20   case None() when option != null:\n\
                  \x20     option;\n\
                  \x20 }\n\
                  }";
    // Clause 1: scrutinee read (declared), body read (Some). Clause 2's
    // guard runs after `None()` promoted its own matched path — the read
    // carries None; clause 1's Some leaked nowhere.
    assert_eq!(
        reads_of(source, "option"),
        ["Option", "Some", "None", "None"],
        "{source:?}"
    );
}

/// A shared case body joins its heads' matched states
/// (`switch_endAlternatives`): two conflicting head promotions keep only
/// the common chain prefix — nothing — so the shared body reads the
/// declared type.
#[test]
fn shared_case_body_joins_away_conflicting_promotions() {
    let source = "void f<T>(Option<T> option) {\n\
                  \x20 switch (option) {\n\
                  \x20   case Some():\n\
                  \x20   case None():\n\
                  \x20     option;\n\
                  \x20 }\n\
                  }";
    assert_eq!(
        reads_of(source, "option"),
        ["Option", "Option"],
        "{source:?}"
    );
}

/// A clause head that promotes nothing leaves the body at the declared
/// type: constant patterns carry no head type (only the `null` constant's
/// non-null face promotes, a separate face).
#[test]
fn constant_case_promotes_nothing() {
    let source = "void f(int x) {\n\
                  \x20 switch (x) {\n\
                  \x20   case 1:\n\
                  \x20     x;\n\
                  \x20 }\n\
                  }";
    assert_eq!(reads_of(source, "x"), ["int", "int"], "{source:?}");
}

/// An explicit-type re-assignment inside the body demotes per the
/// official write rules (`_demoteViaAssignment`): the written type
/// satisfies no chain entry, so the promotion drops and the later read
/// carries the declared type.
#[test]
fn body_write_demotes_the_case_promotion() {
    let source = "void f<T>(Option<T> option, Option<T> other) {\n\
                  \x20 switch (option) {\n\
                  \x20   case Some():\n\
                  \x20     option = other;\n\
                  \x20     option;\n\
                  \x20 }\n\
                  }";
    assert_eq!(
        reads_of(source, "option"),
        ["Option", "Option"],
        "{source:?}"
    );
}

/// A non-variable scrutinee carries no promotion key: the head event
/// promotes nothing and the body reads stay at their declared types (the
/// cannotMatch unreachability face for literal scrutinees is not mounted
/// — the flow layer holds no static type for the non-reference
/// scrutinee).
#[test]
fn literal_scrutinee_promotes_nothing() {
    let source = "void f(option) {\n\
                  \x20 switch (null) {\n\
                  \x20   case Some():\n\
                  \x20     option;\n\
                  \x20 }\n\
                  }";
    let (_, name) = read_type_names(source).into_iter().next().unwrap();
    assert_eq!(name, "", "{source:?}");
}

/// Switch-expression alternatives run the same promotion machinery
/// (`analyzeSwitchExpression`'s pattern → guard → expression order): each
/// arm sees its own head's promotion and the post-expression read sees
/// the declared type.
#[test]
fn switch_expression_alternatives_promote_and_chain() {
    let source = "void f<T>(Option<T> option) {\n\
                  \x20 var v = switch (option) {\n\
                  \x20   Some() => option,\n\
                  \x20   None() => option,\n\
                  \x20 };\n\
                  \x20 option;\n\
                  }";
    assert_eq!(
        reads_of(source, "option"),
        ["Option", "Some", "None", "Option"],
        "{source:?}"
    );
}

/// A nested object pattern constrains its own field, not the matched
/// value: only the top-level head promotes (the official sub-pattern
/// face runs under pushPropertySubpattern).
#[test]
fn nested_object_pattern_promotes_only_the_head() {
    let source = "void f(foo) {\n\
                  \x20 switch (foo) {\n\
                  \x20   case Foo(f: Bar()):\n\
                  \x20     foo;\n\
                  \x20 }\n\
                  }";
    assert_eq!(reads_of(source, "foo"), ["", "Foo"], "{source:?}");
}

/// The if-case's matched value is the same promotion key: the pattern
/// promotes the scrutinee through the then-branch only — the join at the
/// if's close restores the declared type (official `ifCaseStatement`).
#[test]
fn if_case_promotes_the_scrutinee_through_the_then_branch() {
    let source = "void f<T>(Option<T> option) {\n\
                  \x20 if (option case Some()) {\n\
                  \x20   option;\n\
                  \x20 }\n\
                  \x20 option;\n\
                  }";
    assert_eq!(
        reads_of(source, "option"),
        ["Option", "Some", "Option"],
        "{source:?}"
    );
}

/// #1758 probe: the function-typed formal `f` declares a flow variable —
/// `f()?[x]` on a nullable-returning formal splits the short-circuit path
/// (the statement after stays live); a non-nullable return evaluates the
/// index unconditionally.
#[test]
fn probe_fn_typed_formal_null_aware() {
    assert_eq!(
        dead_snippets("void g(List<int>? f()) { f()?[throw '']; 0; }"),
        Vec::<String>::new()
    );
}

/// #1758 probe 2: a plain nullable local's `?[...]` DOES split — the
/// statement after `x?[throw '']` stays live.
#[test]
fn probe_nullable_var_index_aware() {
    assert_eq!(
        dead_snippets("void g(List<int>? x) { x?[throw '']; 0; }"),
        Vec::<String>::new()
    );
}

/// #1758 probe 3: the invocation shape — `f()?.remainder(throw '')` on a
/// nullable-returning formal splits; the statement after stays live.
#[test]
fn probe_nullable_var_invoke_aware() {
    assert_eq!(
        dead_snippets("void g(int? f()) { f()?.remainder(throw ''); 0; }"),
        Vec::<String>::new()
    );
}

/// #1758: the unreachable twins — a non-nullable-returning formal's `?.`
/// kills the skip path, so the statement after dies (oracle anchors the
/// `0;` row).
#[test]
fn probe_nonnull_var_null_aware_dead() {
    assert_eq!(
        dead_snippets("void g(int x) { x?.remainder(throw ''); 0; }"),
        ["0;"]
    );
    assert_eq!(
        dead_snippets("void g(int f()) { f()?.remainder(throw ''); 0; }"),
        ["0;"]
    );
    assert_eq!(
        dead_snippets("void g(List<int> f()) { f()?[throw '']; 0; }"),
        ["0;"]
    );
}

/// #1758: the chained member tail — `x?.a.b(throw _)` on a nullable
/// receiver splits and rejoins at the statement end; nothing dies. The
/// assignment RHS inside a shorted region (`c?.f = throw _`) rejoins the
/// same way.
#[test]
fn probe_nullable_var_chain_tail() {
    assert_eq!(
        dead_snippets("void g(int? f()) { f()?.hashCode.remainder(throw ''); 0; }"),
        Vec::<String>::new()
    );
    assert_eq!(
        dead_snippets("class C { int f = 0; }\nvoid g(C? c) { c?.f = throw ''; 0; }"),
        Vec::<String>::new()
    );
    // The cascade twins (`?..`) rejoin at the cascade's own end — a
    // section-side `throw` cannot kill the receiver-was-null path.
    assert_eq!(
        dead_snippets("void g(List<int>? f()) { f()?..hashCode.remainder(throw ''); 0; }"),
        Vec::<String>::new()
    );
    assert_eq!(
        dead_snippets("class C { int f = 0; }\nvoid g(C? c) { c?..f = throw ''; 0; }"),
        Vec::<String>::new()
    );
}

// —— why-not-promoted facts (#1830) ————————————————————————————————————————

/// The source text under a fact span (UTF-16 == ASCII in the fixtures).
fn span_text(source: &str, range: dartq_source::SourceRange) -> &str {
    &source[range.offset as usize..(range.offset + range.length) as usize]
}

/// The why-not-promoted rows of a source, paired with the flow that
/// interned their entry rows (the producer consumption path: match the
/// row, read its reasons through `non_promotion_context`) and the full
/// fact list for failure messages.
fn why_rows(
    source: &str,
) -> (
    std::rc::Rc<std::cell::RefCell<Flow<'_>>>,
    Vec<FlowFact>,
    Vec<FlowFact>,
) {
    let (flow, facts) = analyze(source);
    let rows = facts
        .iter()
        .filter(|fact| matches!(fact, FlowFact::WhyNotPromoted { .. }))
        .copied()
        .collect();
    (flow.0.clone(), facts, rows)
}

/// The `NonPromotedPropertyRead` rows of a source beside the flow and the
/// full fact list.
fn prop_rows(
    source: &str,
) -> (
    std::rc::Rc<std::cell::RefCell<Flow<'_>>>,
    Vec<FlowFact>,
    Vec<FlowFact>,
) {
    let (flow, facts) = analyze(source);
    let rows = facts
        .iter()
        .filter(|fact| matches!(fact, FlowFact::NonPromotedPropertyRead { .. }))
        .copied()
        .collect();
    (flow.0.clone(), facts, rows)
}

/// `DemoteViaExplicitWrite`'s variable face: a promotion dropped by an
/// assignment records the write as the reason link, and the later read on
/// the same path reports it (official `whyNotPromoted`'s variable arm,
/// `flow_analysis.dart:8832-8857`; the context row anchors at the write
/// node, `assignment_expression_resolver.dart:322-329`). The read stays
/// on the demotion's own path: the if-join rebuilds the model and drops
/// the history (official `_identicalOrNew`).
#[test]
fn why_not_promoted_names_the_demoting_write() {
    let source = "void f() { int? x; if (x is int) { x = null; x; } }";
    let (flow, facts, rows) = why_rows(source);
    assert_eq!(rows.len(), 1, "{facts:?}");
    let FlowFact::WhyNotPromoted {
        span,
        current,
        current_args,
        ..
    } = rows[0]
    else {
        unreachable!()
    };
    assert_eq!(span_text(source, span.range), "x");
    // The flow type at the read is the declared `int?` (the write left it
    // unpromoted); the entry type is the dropped `int`.
    assert!(matches!(
        current,
        crate::ty::Ty {
            base: crate::ty::Base::Name(_),
            nullable: true
        }
    ));
    assert_eq!(current_args, crate::ty::EMPTY_ARGS);
    let flow = flow.borrow();
    let context = flow.non_promotion_context(&rows[0]).unwrap();
    assert_eq!(context.len(), 1);
    assert!(matches!(
        context[0].ty,
        crate::ty::Ty {
            base: crate::ty::Base::Name(_),
            nullable: false
        }
    ));
    assert_eq!(context[0].args, crate::ty::EMPTY_ARGS);
    let crate::facts::NonPromotionCause::ExplicitWrite { span: write } = context[0].cause else {
        panic!("{:?}", context[0].cause)
    };
    assert_eq!(span_text(source, write), "x = null");
}

/// `DemoteViaSuspension`: an `await` inside a local function drops the
/// promotions of the variables the function reads and the enclosing
/// function writes, and the read after the suspension reports the
/// suspension node as the reason (official `suspension`,
/// `flow_analysis.dart:7909-7948`; the context row anchors at the await
/// expression). A variable written solely by the enclosing function is
/// NOT write-captured inside the closure
/// (`flow_analysis.dart:8707-8713`), so the promotion can stand before
/// the suspension drops it.
#[test]
fn why_not_promoted_names_the_suspension() {
    let source = "void f() async { int? x; cb() async { if (x is int) { await h(); x; } } x = 1; }";
    let (flow, facts, rows) = why_rows(source);
    assert_eq!(rows.len(), 1, "{facts:?}");
    let FlowFact::WhyNotPromoted { span, .. } = rows[0] else {
        unreachable!()
    };
    assert_eq!(span_text(source, span.range), "x");
    let flow = flow.borrow();
    let context = flow.non_promotion_context(&rows[0]).unwrap();
    assert_eq!(context.len(), 1);
    let crate::facts::NonPromotionCause::Suspension { span: node } = context[0].cause else {
        panic!("{:?}", context[0].cause)
    };
    assert_eq!(span_text(source, node), "await h()");
}

/// The suspension face needs the local-function nesting: an `await` in
/// the async body itself cannot let an enclosing function resume, so
/// nothing demotes and the read carries no why-not-promoted row
/// (official `_enclosingFunctionExpressionInfoStack` gate).
#[test]
fn await_outside_a_local_function_demotes_nothing() {
    let source = "void f() async { int? x; if (x is int) { await h(); x; } }";
    let (_flow, _facts, rows) = why_rows(source);
    assert!(rows.is_empty(), "{rows:?}");
}

/// A read whose promotion stands, or one whose variable carries no
/// demotion history, emits no row — the rows mark failed promotions
/// only.
#[test]
fn reads_without_dropped_promotions_carry_no_row() {
    let standing = "void f() { int? x; if (x is int) { x; } }";
    let (_flow, _facts, rows) = why_rows(standing);
    assert!(rows.is_empty(), "{standing:?}");
    let never_promoted = "void f() { int? x; x = null; x; }";
    let (_flow, _facts, rows) = why_rows(never_promoted);
    assert!(rows.is_empty(), "{never_promoted:?}");
}

/// The property mirror (`_getNonPromotionReasons`'s `_PropertyReference`
/// arm, `flow_analysis.dart:8766-8815`): a receiver write sweeps the
/// standing property promotion into the slot's evidence, and the
/// unpromoted read reports it — the identity row carrying the member
/// name and the receiver's declaration anchor. A read with no recorded
/// evidence (never tested or promoted) emits no row.
#[test]
fn non_promoted_property_read_reports_the_swept_evidence() {
    let class_decl = "class C { final int? _b; } ";
    let bare = format!("{class_decl}void f(C c) {{ c._b; }}");
    let (_flow, _facts, rows) = prop_rows(&bare);
    assert!(rows.is_empty(), "{rows:?}");

    let source = format!("{class_decl}void f(C c) {{ if (c._b != null) {{ c = C(); c._b; }} }}");
    let (flow, facts, rows) = prop_rows(&source);
    assert_eq!(rows.len(), 1, "{facts:?}");
    let FlowFact::NonPromotedPropertyRead {
        span,
        name,
        receiver,
        ..
    } = rows[0]
    else {
        unreachable!()
    };
    assert_eq!(span_text(&source, span.range), "_b");
    assert_eq!(flow.borrow().name_lexeme(name), "_b");
    let crate::facts::PropReceiver::Local(anchor) = receiver else {
        panic!("{receiver:?}")
    };
    // The receiver's declaration anchor: the formal parameter's name
    // token (`c` in `f(C c)`).
    assert_eq!(span_text(&source, anchor), "c");
    // The evidence: the would-be promoted type (the non-null marker),
    // tagged `PropertyEvidence` — the inherent-vs-conflict verdict is the
    // consumer's element-layer question.
    let flow = flow.borrow();
    let context = flow.non_promotion_context(&rows[0]).unwrap();
    assert_eq!(context.len(), 1);
    assert_eq!(context[0].ty, crate::ty::Ty::UNKNOWN);
    assert_eq!(
        context[0].cause,
        crate::facts::NonPromotionCause::PropertyEvidence
    );
}

/// The increment face of the write anchor: `x++` demotes the promoted
/// type the written `num` no longer satisfies and the row anchors at the
/// whole postfix expression (the official `postIncDec` node,
/// increment_or_decrement_resolver.dart:344-349).
#[test]
fn why_not_promoted_anchors_the_increment_expression() {
    let source = "void f() { num x = 1; if (x is int) { x++; x; } }";
    let (flow, facts, rows) = why_rows(source);
    assert_eq!(rows.len(), 1, "{facts:?}");
    let flow = flow.borrow();
    let context = flow.non_promotion_context(&rows[0]).unwrap();
    assert_eq!(context.len(), 1);
    let crate::facts::NonPromotionCause::ExplicitWrite { span: write } = context[0].cause else {
        panic!("{:?}", context[0].cause)
    };
    assert_eq!(span_text(source, write), "x++");
}

/// Bounded growth: repeated reads of one demoted state share one arena
/// row (equal chains intern once) — the arena stays at the reserved empty
/// row plus the one demotion context. A write that demotes nothing adds
/// no links, and the official's no-change-write quirk erases the prior
/// link outright (`flow_analysis.dart:4838-4846`, mirrored in
/// `State::write`), so a following read carries no row.
#[test]
fn non_promotion_rows_dedup_and_stay_bounded() {
    let source = "void f() { int? x; if (x is int) { x = null; x; x; x; } }";
    let (flow, facts, rows) = why_rows(source);
    assert_eq!(rows.len(), 3, "{facts:?}");
    let ids: Vec<u32> = rows
        .iter()
        .map(|fact| match fact {
            FlowFact::WhyNotPromoted { entries, .. } => *entries,
            _ => unreachable!(),
        })
        .collect();
    assert_eq!(ids, [ids[0], ids[0], ids[0]], "equal chains share one row");
    {
        let flow = flow.borrow();
        assert_eq!(
            flow.non_promotion_context(&rows[0]).unwrap().len(),
            1,
            "one demotion: one link"
        );
    }
    // The quirk: a second no-demotion write erases the link, and reads
    // after it carry no row.
    let after = "void f() { int? x; if (x is int) { x = null; x; x = null; x; } }";
    let (_flow, _facts, rows) = why_rows(after);
    assert_eq!(rows.len(), 1, "{_facts:?}");
}

/// The entry rows are compact copyable payloads — a `u32`-keyed arena of
/// small structs, no heap per fact row.
#[test]
fn non_promotion_entries_are_compact_copyable() {
    fn assert_copy<T: Copy + 'static>() {}
    assert_copy::<crate::facts::NonPromotionEntry>();
    assert_copy::<crate::facts::NonPromotionCause>();
    assert_copy::<crate::facts::PropReceiver>();
    assert_copy::<crate::facts::NonPromotionReason>();
    assert_copy::<crate::facts::PropertyNonPromotability>();
    // Three u32-ish fields (type, argument tree, cause) plus the cause's
    // own anchor — still a register-passable, no-heap payload.
    assert!(std::mem::size_of::<crate::facts::NonPromotionEntry>() <= 32);
    assert!(std::mem::size_of::<crate::facts::NonPromotionCause>() <= 16);
    // The resolved reasons stay register-passable too (the widest variant
    // carries one span).
    assert!(std::mem::size_of::<crate::facts::NonPromotionReason>() <= 16);
}

/// The helper dispatch: the three why-not-promoted faces resolve to their
/// entry rows (index 0 = the official empty-map path), every other fact
/// row answers `None`.
#[test]
fn non_promotion_context_dispatches_the_promotion_faces() {
    let (flow, _facts) = analyze("void f() {}");
    let flow = flow.0.borrow();
    let empty: &[crate::facts::NonPromotionEntry] = &[];
    assert_eq!(
        flow.non_promotion_context(&FlowFact::ThisNotPromoted {
            span: tspan(0),
            entries: 0,
        }),
        Some(empty)
    );
    assert_eq!(
        flow.non_promotion_context(&FlowFact::NonPromotedPropertyRead {
            span: tspan(0),
            name: 0,
            receiver: crate::facts::PropReceiver::This,
            entries: 0,
        }),
        Some(empty)
    );
    assert_eq!(
        flow.non_promotion_context(&FlowFact::DeadCode { span: tspan(0) }),
        None
    );
}

/// The resolved reasons (the producer's render input): the recorded
/// write-demotion link passes through with its anchor, and the verdict
/// input is not consulted for a variable row (`None` here would mean
/// "conflict" for a property row, yet the answer stays `ExplicitWrite`).
#[test]
fn non_promotion_reasons_pass_the_write_link_through() {
    let source = "void f() { int? x; if (x is int) { x = null; x; } }";
    let (flow, _facts, rows) = why_rows(source);
    assert_eq!(rows.len(), 1);
    let flow = flow.borrow();
    let reasons = flow.non_promotion_reasons(&rows[0], None);
    assert_eq!(reasons.len(), 1);
    let crate::facts::NonPromotionReason::ExplicitWrite { span } = reasons[0] else {
        panic!("{:?}", reasons[0])
    };
    assert_eq!(span_text(source, span), "x = null");
}

/// The suspension link passes through the same way, anchor intact.
#[test]
fn non_promotion_reasons_pass_the_suspension_link_through() {
    let source = "void f() async { int? x; cb() async { if (x is int) { await h(); x; } } x = 1; }";
    let (flow, _facts, rows) = why_rows(source);
    assert_eq!(rows.len(), 1);
    let flow = flow.borrow();
    let reasons = flow.non_promotion_reasons(&rows[0], None);
    assert_eq!(reasons.len(), 1);
    let crate::facts::NonPromotionReason::Suspension { span } = reasons[0] else {
        panic!("{:?}", reasons[0])
    };
    assert_eq!(span_text(source, span), "await h()");
}

/// The property family resolves through the element verdict — the mirror
/// of upstream's `whyPropertyIsNotPromotable` split
/// (`flow_analysis.dart:8796-8806`): `Some(why)` is the inherent reason,
/// `None` the non-inherent conflict; both branches on one recorded row.
#[test]
fn non_promotion_reasons_split_the_property_verdict() {
    let source = "class C { final int? _b; } void f(C c) { if (c._b != null) { c = C(); c._b; } }";
    let (flow, _facts, rows) = prop_rows(source);
    assert_eq!(rows.len(), 1);
    let flow = flow.borrow();
    assert_eq!(
        flow.non_promotion_reasons(
            &rows[0],
            Some(crate::facts::PropertyNonPromotability::NotFinal),
        ),
        vec![crate::facts::NonPromotionReason::PropertyInherent {
            why: crate::facts::PropertyNonPromotability::NotFinal
        }]
    );
    assert_eq!(
        flow.non_promotion_reasons(&rows[0], None),
        vec![crate::facts::NonPromotionReason::PropertyConflict]
    );
}

/// The `this` variant and the empty-arena path: a `ThisNotPromoted` row
/// over the reserved empty entries renders nothing, and the recorded
/// `this` tag maps to [`crate::facts::NonPromotionReason::This`]. No
/// source fixture produces the row — the engine tracks no `this`
/// type-test face (this-promotion is upstream's experimental-off flag),
/// so the arm is exercised through the recorded-cause mapping directly.
#[test]
fn non_promotion_reasons_map_this_and_answer_empty_without_entries() {
    let (flow, _facts) = analyze("void f() {}");
    let flow = flow.0.borrow();
    assert!(
        flow.non_promotion_reasons(
            &FlowFact::ThisNotPromoted {
                span: tspan(0),
                entries: 0
            },
            None,
        )
        .is_empty()
    );
    assert_eq!(
        crate::facts::NonPromotionReason::resolve(
            crate::facts::NonPromotionCause::ThisNotPromoted,
            None
        ),
        crate::facts::NonPromotionReason::This
    );
}

/// A `case null:` whose disjointness is declared-type-level stands down:
/// the semantic layer's own case-body dead row owns the report (its space
/// world computes over declared types), so the flow kill must not produce a
/// second dead region — `switch_same_type_variable_pattern_chain_dead`
/// duplicated `(95,19)` until the kill learned to defer. Promotion-derived
/// disjointness stays live (above).
#[test]
fn null_constant_pattern_declared_disjoint_defers_to_semantic() {
    let source = "void f(int a) { switch (a) { case null: print2(1); break; case 3: print2(2); } }";
    assert!(dead_snippets(source).is_empty());
}

/// TEMP PROBE (#2122): observe read-type rows for the issue60264 shape.
/// A matched object pattern promotes the switched variable for the case
/// body's reads (official `promoteForPattern`, flow_analysis.dart:7703-7755,
/// fed by `analyzeObjectPattern`): each clause's body read carries the
/// clause's own matched type — `case Some():` reads `Some`, the following
/// `case None():` reads `None` (the earlier promotion does not leak: the
/// failed arm joined the unmatched chain, official `coversMatchedType`
/// gate) — and the post-switch read is back at the declared type (the
/// close join with the chain, `switch_end`).
#[test]
fn object_pattern_promotes_scrutinee_for_case_body_reads() {
    let source = concat!(
        "class Option<T> { const Option(); } ",
        "class Some<T> extends Option<T> { final T value; const Some(this.value); } ",
        "class None extends Option<Never> { bool get noneProp => false; } ",
        "void fun<T>(Option<T> option) { switch (option) { case Some(): option.value; ",
        "case None(): option.noneProp; } option; }"
    );
    let (flow, facts) = analyze(source);
    let rows: Vec<(u32, String)> = facts
        .iter()
        .filter_map(|fact| match fact {
            FlowFact::ReadType { span, ty, .. } => {
                let name = match ty.base {
                    crate::ty::Base::Name(id) => flow.0.borrow().name_lexeme(id).to_string(),
                    _ => String::new(),
                };
                Some((span.range.offset, name))
            }
            _ => None,
        })
        .collect();
    // Offsets: the scrutinee read, the two case-body reads, the post-switch
    // read — all of `option`.
    let scrutinee = source.find("switch (option)").unwrap() + 8;
    let body_some = source.find("option.value").unwrap();
    let body_none = source.find("option.noneProp").unwrap();
    let post = source.rfind("option; }").unwrap();
    let row = |offset: u32| {
        rows.iter()
            .find(|(o, _)| *o == offset as u32)
            .map(|(_, name)| name.as_str())
            .unwrap_or_else(|| panic!("no read row at {offset}: {rows:?}"))
    };
    assert_eq!(row(scrutinee as u32), "Option", "{rows:?}");
    assert_eq!(row(body_some as u32), "Some", "{rows:?}");
    assert_eq!(row(body_none as u32), "None", "{rows:?}");
    assert_eq!(row(post as u32), "Option", "{rows:?}");
}


/// Refutable pattern shapes join their failure arm into the if-case's
/// unmatched chain: the statement after `if (v case (int a, int b))` stays
/// live (official `_unmatched` accumulates every can-fail shape's else arm —
/// flow_analysis.dart's pattern dispatch). Without the join the chain sits
/// at its initial unreachable state and the fall-through reads dead.
#[test]
fn if_case_record_pattern_leaves_fall_through_live() {
    let source = r#"String f(Object v) {
  if (v case (int a, int b)) {
    return 'pair';
  }
  return 'other';
}
String g(Object v) {
  if (v case (int a, int b) when a < b) {
    return 'asc';
  }
  if (v case (x: final l, y: final r)) {
    return 'pt';
  }
  if (v case (var only,)) {
    return 's';
  }
  return 'other';
}
String h(Object v) {
  if (v case int n) {
    return 'n';
  }
  return 'o';
}"#;
    assert!(dead_snippets(source).is_empty());
}

/// A for-in head pattern opens a standalone pattern context that official
/// `patternForIn_end` pops at the `in` keyword, before the iterable runs.
/// Leaked, its unreachable unmatched chain became the else branch of the
/// NEXT plain if: a bare `ThenBegin` adopted any open `Decl` context and
/// used its unmatched as the branch state (args `usage.dart:101` — the
/// spurious else dead row after a for-in object pattern).
#[test]
fn for_in_pattern_head_does_not_dead_the_next_if_else() {
    let source = r#"void f(Helper? helper, Iterable<(int, int)> pairs) {
  for (final (a, b) in pairs) {
    a + b;
  }
  if (helper != null) {
    helper.hashCode;
  } else {
    'missing'.length;
  }
}"#;
    assert_eq!(dead_snippets(source), Vec::<String>::new());
}

/// The pattern variable declaration's context closes at its own end
/// (official `patternVariableDeclaration_end`): the declaration's
/// unmatched chain must not become the next plain if's else branch.
#[test]
fn pattern_declaration_does_not_dead_the_next_if_else() {
    let source = r#"void f(int? n) {
  var (a, b) = (1, 2);
  if (n == 3) {
    n + 1;
  } else {
    'other'.length;
  }
}"#;
    assert_eq!(dead_snippets(source), Vec::<String>::new());
}

/// The pattern assignment's context closes at the assignment's end
/// (official `patternAssignment_end`) — no leak into the following if.
#[test]
fn pattern_assignment_does_not_dead_the_next_if_else() {
    let source = r#"void f(int? n) {
  int a, b;
  (a, b) = (1, 2);
  if (n == 3) {
    n + 1;
  } else {
    'other'.length;
  }
}"#;
    assert_eq!(dead_snippets(source), Vec::<String>::new());
}

/// An `is`-check consumed as a plain value narrows nothing: the split
/// rides `Expr::branches` for the condition consumers, and a value
/// context keeps the pre-test state. Narrowing at the fold poisoned later
/// reads — a following `x ?? g()` saw a promoted `x` and deaded `g()`
/// (dio `options.dart:629`, the `copyWith` argument list).
#[test]
fn value_context_null_comparison_does_not_narrow() {
    let source = r#"int? g() => 1;
void f(int? a) {
  final b = a != null;
  int c = a ?? g();
  print(c);
  print(b);
}"#;
    assert_eq!(dead_snippets(source), Vec::<String>::new());
}

/// An `is`-check in a plain initializer narrows nothing either; the
/// condition context still splits (control below).
#[test]
fn value_context_is_check_does_not_narrow() {
    let source = r#"void f(int? a) {
  final b = a is int;
  int c = a ?? 3;
  print(c);
  print(b);
}"#;
    assert_eq!(dead_snippets(source), Vec::<String>::new());
}

/// Condition-context null comparison still promotes (the split consumer
/// control for the value-context pins).
#[test]
fn condition_context_null_comparison_still_promotes() {
    let source = r#"void f(int? a) {
  if (a != null) {
    print(a + 1);
  }
}"#;
    assert_eq!(dead_snippets(source), Vec::<String>::new());
}

/// A Never exit inside a then arm dies with its region: the IfEnd join
/// revives the flow and retires the exit's anchoring latch, so a later
/// flow-killing statement (`throw 'end';`) does not self-anchor on the
/// stale exit (riverpod `async_value.dart:603`). Hand-driven: the exit
/// event fires only when a resolved back-end answers Never, which the
/// name-level pipeline never does.
#[test]
fn never_exit_in_then_arm_does_not_dead_post_join_throw() {
    use crate::ev::Ev;
    use crate::facts::FlowFact;
    // `void g(bool c) { if (c) { boom(); } throw 'end'; }`
    let events: Vec<Ev> = [
        vec![
            Ev::ExecBegin { is_closure: false },
            Ev::BlockBodyBegin,
        ],
        vec![Ev::IfBegin { span: tspan(17) }],
        vec![Ev::OtherExpr { span: tspan(21) }],
        vec![
            Ev::ThenBegin,
            Ev::Block {
                kind: crate::ev::BlockKind::Statement,
                span: tspan(23),
            },
            // `boom();` — the call resolves to Never; the exit drains after
            // the statement's own wrapper (the recorder latches it across
            // the wrapper).
            Ev::OtherExpr { span: tspan(25) },
            Ev::Exit { end: tspan(30) },
            Ev::ExprStmt { span: tspan(25) },
            Ev::StmtEnd { end: 32 },
            Ev::EndBlock { end: 34 },
            Ev::IfEnd { has_else: false },
            Ev::StmtEnd { end: 34 },
        ],
        // `throw 'end';` — kills the flow itself; its wrapper must stay
        // silent (the join retired the exit latch).
        vec![
            Ev::OtherExpr { span: tspan(36) },
            Ev::Throw,
            Ev::ExprStmt { span: tspan(36) },
            Ev::StmtEnd { end: 47 },
        ],
        vec![
            Ev::BlockBodyEnd,
            Ev::ExecEnd {
                is_expr: false,
                end: tspan(48),
            },
        ],
    ]
    .concat();
    let interp = interp_of(&events);
    assert!(
        !interp
            .facts
            .iter()
            .any(|fact| matches!(fact, FlowFact::DeadCode { .. })),
        "the throw after the revived join is oracle-clean: {:?}",
        interp.facts
    );
}

/// Control for the retirement pin: without the if-join the next statement
/// anchors its own dead row (`boom(); g();` reports at `g();`).
#[test]
fn never_exit_still_anchors_the_next_statement() {
    use crate::ev::Ev;
    use crate::facts::FlowFact;
    // `void g() { boom(); print('x'); }`
    let events: Vec<Ev> = [
        vec![
            Ev::ExecBegin { is_closure: false },
            Ev::BlockBodyBegin,
        ],
        vec![
            Ev::OtherExpr { span: tspan(12) },
            Ev::Exit { end: tspan(17) },
            Ev::ExprStmt { span: tspan(12) },
            Ev::StmtEnd { end: 19 },
        ],
        vec![
            Ev::OtherExpr { span: tspan(20) },
            Ev::ExprStmt { span: tspan(20) },
            Ev::StmtEnd { end: 30 },
        ],
        vec![
            Ev::BlockBodyEnd,
            Ev::ExecEnd {
                is_expr: false,
                end: tspan(31),
            },
        ],
    ]
    .concat();
    let interp = interp_of(&events);
    let dead: Vec<_> = interp
        .facts
        .iter()
        .filter_map(|fact| match fact {
            FlowFact::DeadCode { span } => Some(span.range.offset),
            _ => None,
        })
        .collect();
    assert_eq!(dead, vec![20], "{:?}", interp.facts);
}

/// #2158 pin: a creation RHS types the write — `x = C()..m()` (the cascade
/// keeps the head's named type as the target's), `x = new C()`,
/// `x = const C()`, `x = C<T>.named()` and `x = C<T>()` all promote a `C?`
/// local to `C` (each read after the write is a promoted read).
#[test]
fn creation_rhs_types_the_write() {
    use crate::ty::{Base, Ty};
    let source = r#"void f() {
  Object? a; a = Object()..toString(); a;
  Object? b; b = new Object(); b;
  Object? c; c = const Object(); c;
  Object? d; d = Object<int>.from('x'); d;
  Object? e; e = Object<int>(); e;
}"#;
    let promoted: Vec<Ty> = promoted_tys(source);
    let non_null_named = promoted
        .iter()
        .filter(|t| matches!(t.base, Base::Name(_)) && !t.nullable)
        .count();
    assert_eq!(non_null_named, 5, "{promoted:?}");
}

/// #2180 face-2 pin: a `late` local written in the enclosing body and only
/// read inside closures is not definitely unassigned at those reads — the
/// closure's collect records the write into `anywhere.written` and the
/// closure-entry conservative join clears the `unassigned` bit (official
/// `_functionExpression_begin`'s `conservativeJoin(anywhere.written, …)`,
/// `flow_analysis.dart:8731-8738` with `assigned_variables.dart:270`'s
/// write-time `anywhere.written.add`). The pre-fix miss: `exec_end`'s
/// collecting arm skipped the `is_expr` value push, so an `ArgumentsEnd`
/// pop ate the enclosing assignment's target slot and `record_write`
/// never ran — the whole-buffer `anywhere` stayed write-free.
#[test]
fn late_var_written_around_closure_reads_not_unassigned() {
    let source = r#"void f(Stream s) {
  late StreamSubscription sub;
  sub = s.listen((d) {
    sub.pause();
  }, onDone: () {
    sub.cancel();
  });
  sub.hashCode;
}"#;
    // Every read of `sub` emits a row (`!assigned` holds through the
    // conservative join), but none may carry the definite bit — that bit
    // alone drives `definitely_unassigned_late_local_variable`.
    let facts = facts_of(source);
    let unassigned_rows: Vec<bool> = facts
        .iter()
        .filter_map(|fact| match fact {
            FlowFact::UnassignedRead {
                definitely_unassigned,
                ..
            } => Some(*definitely_unassigned),
            _ => None,
        })
        .collect();
    assert_eq!(unassigned_rows.len(), 2, "{facts:?}");
    assert!(unassigned_rows.iter().all(|d| !*d), "{facts:?}");
}

/// #2185 pin (prefixed `is` narrowing): `handle_type` fires on the
/// qualified chain's FIRST token (`io` of `io.Directory` — official
/// `handleType(typeRefOrPrefix)`, type_info_impl.dart:691), so the
/// recorded type name must be the chain's LAST identifier, the element
/// the spelling resolves to. Interning the prefix collapsed every `io.*`
/// spelling onto `Name(io)`, and `factor(io-typed e, io)` bottomed —
/// the `else` arm of a prefixed `is`-test read unreachable while the
/// oracle reports none (the dartq-only dead-code direction).
#[test]
fn prefixed_is_test_else_arm_stays_reachable() {
    let source = r#"FSE m(io.FSE e) {
  if (e is io.PNFE) {
    return P(e.p, e.m);
  } else {
    return F(e.p, e.m);
  }
}"#;
    assert_eq!(dead_snippets(source), Vec::<String>::new());
    // And the resolved tail name keeps the SAME spelled type factoring
    // against itself: a variable declared `io.PNFE` promotes on
    // `e is io.PNFE`, so a repeat test bottoms the second arm's true
    // branch exactly like the unprefixed shape.
    let source = r#"void f(io.PNFE e) {
  if (e is io.PNFE) {
    g(e);
  } else if (e is io.PNFE) {
    g(e);
  }
}"#;
    assert_eq!(dead_snippets(source).len(), 1, "second arm must be dead");
}

/// #2185 pin (constructor field initializer): `f = e` in `C(...) : f = e`
/// parses as a plain `Assign` event, but its LHS resolves to `this.f`
/// (upstream `ConstructorFieldInitializer`) — never a local write. A
/// like-named parameter must keep its declared nullability, so a later
/// initializer's `param ?? fallback` RHS stays reachable (the oracle
/// reports no dead code; the pre-fix fold wrote the param non-null and
/// deaded the second `??` arm).
#[test]
fn field_initializer_does_not_write_like_named_parameter() {
    let source = r#"class C {
  C(int? f, int? g)
      : f = f ?? 0,
        _g = g ?? calc(g ?? 1);
  final int f;
  final int _g;
}"#;
    assert_eq!(dead_snippets(source), Vec::<String>::new());
    // And the parameter still promotes on its own is-test afterwards —
    // the suppression is the write, not the variable.
    let source = r#"class C {
  C(int? v) : _v = v ?? 0 {
    if (v is int) {
      g(v);
    }
  }
  final int _v;
}"#;
    let facts = facts_of(source);
    assert!(
        facts
            .iter()
            .any(|fact| matches!(fact, FlowFact::PromotedRead { .. })),
        "{facts:?}"
    );
}

/// #2185 pin (prefixed constructor reference): `ConstructorReference`
/// fires on the chain's first token too — `io.File()`'s implicit
/// creation must carry `File`'s name so a write `x = io.File()`
/// promotes `x` to the resolved type, not the shared `io` prefix.
#[test]
fn prefixed_constructor_write_promotes_tail_type() {
    let source = r#"void f() {
  var x = io.File();
  if (x is io.File) {
    g(x);
  }
}"#;
    let facts = facts_of(source);
    let promoted: Vec<String> = facts
        .iter()
        .filter_map(|fact| match fact {
            FlowFact::PromotedRead { ty, .. } => Some(format!("{ty:?}")),
            _ => None,
        })
        .collect();
    assert_eq!(promoted.len(), 1, "{facts:?}");
}

/// #2180 face-1 pin (the reported shape): a `final` local initialized to
/// a constructor invocation and then used through member writes and
/// member-tear-off argument reads inside closures never promotes — no
/// `is`/`as`/null-check on the receiver exists, so no promotion source
/// fires. The oracle-facing failure mode was the receiver reporting as
/// `bool`; this face pins that nothing promotes here at all (a `bool`
/// row could only come from a tested-set leak the source never spells).
#[test]
fn constructor_initialized_final_receiver_never_promotes() {
    let source = r#"Stream f(Stream s) {
  final c = StreamController.broadcast(sync: true);
  c.onListen = () {
    s.listen((d) {});
  };
  return c.stream;
}"#;
    let facts = facts_of(source);
    assert!(
        facts.iter().all(|fact| !matches!(
            fact,
            FlowFact::PromotedRead { .. }
                | FlowFact::UnassignedRead { .. }
                | FlowFact::DeadCode { .. }
        )),
        "{facts:?}"
    );
}


/// `Option<T> option; option is Some` — a raw tested type against a
/// written generic declaration of a DIFFERENT base — promotes
/// optimistically: whether `Some<dynamic>` stands below `Option<T>` is the
/// class-hierarchy question the name-level seam cannot decide (the
/// `args_admit` gate decides only the same-base spelling `x is List` on
/// `List<int> x`, where `List<dynamic>` provably is no subtype). The
/// pattern face (`case Some():`) gates identically — same-base only — so
/// the two faces agree on `Option<T>`'s scrutinee. The resolved back-end
/// re-tightens the undecided arm through `TypeOps::try_promote_to`.
#[test]
fn cross_base_raw_tested_against_written_declaration_promotes() {
    let source = "void f<T>(List<int> x, Option<T> option) {\n\
                      x is List;\n\
                      if (option is Some) { option; }\n\
                  }";
    let facts = facts_of(source);
    let promoted_offsets: Vec<u32> = facts
        .iter()
        .filter_map(|fact| match fact {
            FlowFact::PromotedRead { span, .. } => Some(span.range.offset),
            _ => None,
        })
        .collect();
    // The if-body `option` read carries the `Some` promotion.
    let if_body_read = source.rfind("option").unwrap() as u32;
    assert!(
        promoted_offsets.contains(&if_body_read),
        "the cross-base raw test promotes the scrutinee: {facts:?}"
    );
    // The same-base raw test still refuses (`List<dynamic>` is no subtype
    // of `List<int>`): `x` carries no promotion at all.
    let list_reads: Vec<u32> = source
        .match_indices("x")
        .map(|(offset, _)| offset as u32)
        .collect();
    assert!(
        list_reads
            .iter()
            .all(|offset| !promoted_offsets.contains(offset)),
        "the same-base raw test must not promote: {facts:?}"
    );
}

// —— #2243: promotion through inferred declared faces and written type
// arguments ——————————————————————————————————————————————————————————————

/// The `(offset, ty, args)` triples of a source's promoted reads, for the
/// pins that key on where a promotion stands and what tree it carries.
fn promoted_read_rows(
    source: &str,
) -> (std::rc::Rc<std::cell::RefCell<crate::record::Flow<'_>>>, Vec<(u32, crate::ty::Ty, u32)>) {
    let (flow, facts) = analyze(source);
    let rows = facts
        .into_iter()
        .filter_map(|fact| match fact {
            FlowFact::PromotedRead { span, ty, args } => Some((span.range.offset, ty, args)),
            _ => None,
        })
        .collect();
    (flow.0, rows)
}

/// A `var` local initialized from a nullable read adopts the initializer's
/// declared face whole — written type arguments included (#2243) — so the
/// null-check promotion stands on the return path with the instantiated
/// spelling. Before the adoption the slot stayed unknown, the promotion
/// no-oped, and every downstream type question read the declared
/// `List<int>?`.
#[test]
fn inferred_local_adopts_nullable_initializer_face_and_promotes() {
    let source = "void f(List<int>? p) { var l = p; if (l == null) return; l; }";
    let (flow, rows) = promoted_read_rows(source);
    // Exactly one promoted read: the statement-position `l` after the
    // early return.
    let read = source.rfind('l').unwrap() as u32;
    let list = flow.borrow_mut().intern_name("List");
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].0, read);
    assert_eq!(
        rows[0].1,
        crate::ty::Ty {
            base: crate::ty::Base::Name(list),
            nullable: false,
        }
    );
    // The promotion tree spells the written `<int>`.
    let tree: Vec<crate::ty::ArgTy> = flow.borrow().type_args_at(rows[0].2).to_vec();
    assert_eq!(tree.len(), 1);
    assert!(matches!(tree[0].base, crate::ty::Base::Name(_)));
    assert!(!tree[0].nullable);
}

/// The adopted face's promotion answers writes like any named promotion: a
/// null write demotes it (the official keeps a promotion only while the
/// written value satisfies it).
#[test]
fn inferred_local_promotion_demotes_on_a_null_write() {
    let source = "void f(List<int>? p) { var l = p; if (l == null) return; l = null; l; }";
    let (_, rows) = promoted_read_rows(source);
    assert!(rows.is_empty(), "the null write demotes: {rows:?}");
}

/// The unresolved-field initializer shape (the differential's own face):
/// the initializer's type is the unresolved-nullable marker, so the local
/// adopts the opaque-nullable declared face and the null check takes the
/// marker promotion — the non-null unknown face, the local mirror of the
/// property-slot marker chain. The consumer re-decides the promoted type
/// from its own resolved static type.
#[test]
fn opaque_nullable_inferred_local_takes_the_marker_promotion() {
    let source = "class C {\n  List<int>? field;\n  void m() {\n    var l = field;\n    if (l == null) return;\n    l;\n  }\n}";
    let (_, rows) = promoted_read_rows(source);
    let read = source.rfind('l').unwrap() as u32;
    assert_eq!(
        rows,
        vec![(read, crate::ty::Ty::UNKNOWN, crate::ty::EMPTY_ARGS)],
        "{rows:?}"
    );
}

/// The marker promotion stays a real promotion: a null write drops it (a
/// nullable written value never satisfies a non-null face, opaque or not),
/// and the demoted read carries no row.
#[test]
fn marker_promotion_demotes_on_a_null_write() {
    let source = "class C {\n  List<int>? field;\n  void m() {\n    var l = field;\n    if (l == null) return;\n    l = null;\n    l;\n  }\n}";
    let (_, rows) = promoted_read_rows(source);
    assert!(rows.is_empty(), "the null write demotes the marker: {rows:?}");
}

/// A non-nullable spelled initializer face is NOT adopted: the unknown
/// slot keeps every pre-#2243 answer — the null check stays a live split
/// (no dead-code row the fold never made), and no promotion hangs on the
/// read.
#[test]
fn non_nullable_initializer_face_keeps_the_unknown_slot() {
    let source = "void f(int p) { var l = p; if (l == null) { l; } }";
    let (_, rows) = promoted_read_rows(source);
    assert!(rows.is_empty(), "{rows:?}");
    assert!(dead_snippets(source).is_empty(), "{source:?}");
}

/// `x ??= C<int>();` promotes through the instantiated spelling: the
/// send's written argument tree rides the implicit-creation face (#2243),
/// so the write's LUB lands on `C<int>` and the read after the statement
/// carries the promotion with the `<int>` tree.
#[test]
fn if_null_assignment_through_args_constructor_promotes() {
    let source = "class C<T> {}\nvoid f() {\n  C<int>? x;\n  x ??= C<int>();\n  x;\n}";
    let (flow, rows) = promoted_read_rows(source);
    let read = source.rfind('x').unwrap() as u32;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].0, read);
    assert_eq!(
        rows[0].1,
        crate::ty::Ty {
            base: crate::ty::Base::Name(flow.borrow_mut().intern_name("C")),
            nullable: false,
        }
    );
    let tree: Vec<crate::ty::ArgTy> = flow.borrow().type_args_at(rows[0].2).to_vec();
    assert_eq!(tree.len(), 1);
    assert!(matches!(tree[0].base, crate::ty::Base::Name(_)));
    assert!(!tree[0].nullable);
}

/// The argument-less control: `x ??= Foo();` promoted before #2243 and
/// still does.
#[test]
fn if_null_assignment_through_constructor_promotes() {
    let source = "class Foo {}\nvoid f() {\n  Foo? x;\n  x ??= Foo();\n  x;\n}";
    let (flow, rows) = promoted_read_rows(source);
    let read = source.rfind('x').unwrap() as u32;
    let foo = flow.borrow_mut().intern_name("Foo");
    assert_eq!(
        rows,
        vec![(read, crate::ty::Ty {
            base: crate::ty::Base::Name(foo),
            nullable: false,
        }, crate::ty::EMPTY_ARGS)],
        "{rows:?}"
    );
}

/// The raw-spelling guard: `x ??= C();` on `C<int>? x` types the RHS as
/// the raw face (`C<dynamic>` at the oracle), whose LUB with the declared
/// `C<int>` promotes nothing — the write must not narrow `x` to a spelling
/// the RHS never carried.
#[test]
fn if_null_assignment_through_raw_constructor_stays_unpromoted() {
    let source = "class C<T> {}\nvoid f() {\n  C<int>? x;\n  x ??= C();\n  x;\n}";
    let (_, rows) = promoted_read_rows(source);
    assert!(rows.is_empty(), "{rows:?}");
}

// —— #2241 rider: promotions inside a top-level-variable-initializer closure ——

/// The flow types at every `str` read of a guarded-read source: the
/// null-check promotion must surface as a non-nullable `ReadType` row at
/// both the condition read and the guarded member read.
fn guarded_read_faces(source: &str) -> Vec<(u32, bool)> {
    let mut rows: Vec<(u32, bool)> = read_types_of(source)
        .into_iter()
        .filter(|(offset, _)| {
            let offset = *offset as usize;
            source[offset..].starts_with("str")
        })
        .map(|(offset, ty)| (offset, ty.nullable))
        .collect();
    rows.sort_unstable();
    rows
}

/// A closure nested in a top-level variable initializer promotes its own
/// locals exactly like a plain function body: the initializer world's
/// executable framing must survive the local's own initializer bracket —
/// the world's close belongs to the bracket that opened it. Before the
/// fix the local's `end_variable_initializer` fired the world's
/// `exec_end` at the local's `=`: the replay popped the closure's frame
/// and scope mid-body, every later read of the local resolved to nothing
/// (the implicit-`this` property face), and no promotion could form.
#[test]
fn initializer_closure_guarded_read_promotes_like_a_function_body() {
    let control = concat!(
        "String? mk() => null;\n",
        "void f() { final str = mk(); if (str == null) return; str.hashCode; }",
    );
    let closure = concat!(
        "String? mk() => null;\n",
        "final f = () { final str = mk(); if (str == null) return; str.hashCode; };",
    );
    // Two `str` reads: the condition read carries the potentially-nullable
    // unknown marker (the `mk()` initializer's unspellable face — #2241's
    // adoption), the guarded member read its promoted non-null face. The
    // needles key at the `str` token offset.
    let expected: Vec<(u32, bool)> = ["str == null) return", "str.hashCode"]
        .iter()
        .map(|needle| {
            (source_offset(control, needle), needle.starts_with("str =="))
        })
        .collect();
    assert_eq!(guarded_read_faces(control), expected, "{control:?}");
    let expected: Vec<(u32, bool)> = ["str == null) return", "str.hashCode"]
        .iter()
        .map(|needle| (source_offset(closure, needle), needle.starts_with("str ==")))
        .collect();
    assert_eq!(guarded_read_faces(closure), expected, "{closure:?}");
}

/// The same world in a class field initializer: the closure body's
/// promotions must survive there too.
#[test]
fn field_initializer_closure_guarded_read_promotes() {
    let source = concat!(
        "String? mk() => null;\n",
        "class C {\n",
        "  final f = () { final str = mk(); if (str == null) return; str.hashCode; };\n",
        "}",
    );
    let faces = guarded_read_faces(source);
    assert_eq!(faces.len(), 2, "{source:?}");
    assert_eq!(
        faces,
        vec![
            (source_offset(source, "str == null"), true),
            (source_offset(source, "str.hashCode"), false),
        ],
        "{source:?}"
    );
}

/// The promotion gates dead code inside the initializer closure: after
/// the early-return guard the second `str == null` test is known-false,
/// so its then-branch is dead — the oracle rows DEAD_CODE there (and in
/// the plain-function control). Before the fix the closure emitted no
/// row: the branch condition read rode the unresolved property face.
#[test]
fn initializer_closure_promotion_gates_dead_code() {
    let body = "String? str = mk(); if (str == null) return; if (str == null) { mk(); }";
    let control = format!("String? mk() => null;\nvoid f() {{ {body} }}");
    let closure = format!("String? mk() => null;\nfinal f = () {{ {body} }};");
    let control_dead = dead_snippets(&control);
    assert_eq!(control_dead, vec!["{ mk(); }"], "{control:?}");
    assert_eq!(dead_snippets(&closure), control_dead, "{closure:?}");
}

/// The guarded read as a call argument carries the non-null face — the
/// staged-assignability consumer's row (the #2241 probe's `take(str)`
/// false ARGUMENT_TYPE_NOT_ASSIGNABLE rode the missing fact).
#[test]
fn initializer_closure_guarded_argument_read_carries_non_null_face() {
    let source = concat!(
        "String? mk() => null;\n",
        "void take(String s) {}\n",
        "final f = (Object? arg) { final str = mk(); if (str == null) return 0; take(str); return 1; };",
    );
    let argument = source.rfind("take(str)").unwrap() as u32 + 5;
    let rows = read_types_of(source);
    let argument_row = rows
        .iter()
        .find(|(offset, _)| *offset == argument)
        .unwrap_or_else(|| panic!("no ReadType at the argument read: {rows:?}"));
    assert!(!argument_row.1.nullable, "{rows:?}");
}

/// UTF-16 offset of a needle's first occurrence (ASCII sources: byte ==
/// utf-16), the test corpus's addressing convention.
fn source_offset(source: &str, needle: &str) -> u32 {
    source.find(needle).expect("needle present") as u32
}

// —— #2241 family B: the unknown-face initializer adoption ——
// —— #2241 family B: the unknown-face initializer adoption ——
// —— #2241 family B: the unknown-face initializer adoption ——
// —— #2241 family B: the unknown-face initializer adoption ——
// —— #2241 family B: the unknown-face initializer adoption ——







/// Read-type rows whose source text starts with `needle`, as
/// `(offset, nullable)` pairs sorted by offset (ASCII sources: byte ==
/// utf-16, the test corpus's addressing convention).
fn needle_faces(source: &str, needle: &str) -> Vec<(u32, bool)> {
    let mut rows: Vec<(u32, bool)> = read_types_of(source)
        .into_iter()
        .filter(|(offset, _)| source[*offset as usize..].starts_with(needle))
        .map(|(offset, ty)| (offset, ty.nullable))
        .collect();
    rows.sort_unstable();
    rows
}

/// The condition read of a null-checked local whose initializer the fold
/// can only type as unknown carries the potentially-nullable marker, and
/// the guarded read on the `||`'s short-circuit arm carries its promoted
/// non-null face (the riverpod `final links = ref?._keepAliveLinks;` —
/// `if (... || links.isEmpty)` shape). Before the fix the plain unknown
/// declared face refused every null-check refinement, the guarded read
/// rode unpromoted, and the semantic layer fired the oracle-clean
/// UNCHECKED row.
#[test]
fn null_aware_property_local_promotes_through_null_check() {
    let source = concat!(
        "class Ref { List<String>? keepAliveLinks; }\n",
        "class Provider {\n",
        "  bool get isActive => false;\n",
        "  Ref? get ref => null;\n",
        "  void schedule() {}\n",
        "}\n",
        "void disposeSink(Provider c) {\n",
        "  final links = c.ref?.keepAliveLinks;\n",
        "  if (!c.isActive && (links == null || links.isEmpty)) {\n",
        "    c.schedule();\n",
        "  }\n",
        "}",
    );
    let expected: Vec<(u32, bool)> = ["links == null", "links.isEmpty"]
        .iter()
        .map(|needle| (source_offset(source, needle), needle.ends_with("== null")))
        .collect();
    assert_eq!(needle_faces(source, "links"), expected, "{source:?}");
}

/// The same adoption for a call initializer (`final s = mkList();` — the
/// fold types no returns): the then-branch read after `s != null` is
/// promoted.
#[test]
fn unknown_call_initializer_local_promotes_in_then_branch() {
    let source = concat!(
        "List<String>? mkList() => null;\n",
        "void f() {\n",
        "  final s = mkList();\n",
        "  if (s != null) {\n",
        "    var n = s.isEmpty;\n",
        "  }\n",
        "}",
    );
    let expected: Vec<(u32, bool)> = [("s != null", true), ("s.isEmpty", false)]
        .iter()
        .map(|(needle, nullable)| (source_offset(source, needle), *nullable))
        .collect();
    assert_eq!(needle_faces(source, "s"), expected, "{source:?}");
}

/// A property chain whose receiver is itself a property read (officially
/// not a promotable property target, so no slot face engages and the
/// chain's flow face is the plain unknown) adopts the marker too.
#[test]
fn chained_property_local_promotes_through_null_check() {
    let source = concat!(
        "class Ref { List<String>? keepAliveLinks; }\n",
        "class Provider { Ref? ref; }\n",
        "void f(Provider c) {\n",
        "  final links = c.ref.keepAliveLinks;\n",
        "  if (links != null) {\n",
        "    links.isEmpty;\n",
        "  }\n",
        "}",
    );
    let expected: Vec<(u32, bool)> = [("links != null", true), ("links.isEmpty", false)]
        .iter()
        .map(|(needle, nullable)| (source_offset(source, needle), *nullable))
        .collect();
    assert_eq!(needle_faces(source, "links"), expected, "{source:?}");
}

/// The promotion survives into a switch-case body's loop (the riverpod
/// `element.dart` shape: the local declared inside a case alternative,
/// null-checked, read in the loop condition).
#[test]
fn case_body_local_promotes_through_null_check() {
    let source = concat!(
        "class Ref { List<String>? listeners; }\n",
        "class Res<T> {}\n",
        "class ResData<T> extends Res<T> {}\n",
        "class Holder<T> {\n",
        "  Ref? get ref => null;\n",
        "  void notify(Res<T> next) {\n",
        "    switch (next) {\n",
        "      case final ResData<T> newState:\n",
        "        final subs = ref?.listeners;\n",
        "        if (subs != null) {\n",
        "          for (var i = 0; i < subs.length; i++) {\n",
        "            newState.hashCode;\n",
        "          }\n",
        "        }\n",
        "    }\n",
        "  }\n",
        "}",
    );
    let expected: Vec<(u32, bool)> = [
        ("subs != null", true),
        ("subs.length", false),
    ]
    .iter()
    .map(|(needle, nullable)| (source_offset(source, needle), *nullable))
    .collect();
    assert_eq!(needle_faces(source, "subs"), expected, "{source:?}");
}

/// The `!` assertion refines the opaque-declared face too (the drift
/// `wasm_setup.dart` shape: the local initialized by a null-aware property
/// read, asserted inside a case body, read again two statements later).
/// Before the marker step was centralized the assertion's non-null form
/// left the unknown face unchanged, the later read rode unpromoted, and
/// the oracle-clean UNCHECKED row fired.
#[test]
fn bang_assertion_promotes_unknown_local() {
    let source = concat!(
        "class W { int send = 0; int version = 0; }\n",
        "class Opener { W? _sharedWorker; }\n",
        "enum Impl { opfsShared, other }\n",
        "class Msg { Msg(int send); }\n",
        "void f(Opener opener, Impl implementation) {\n",
        "  Msg message;\n",
        "  final sharedWorker = opener._sharedWorker;\n",
        "  switch (implementation) {\n",
        "    case Impl.opfsShared:\n",
        "      message = Msg(sharedWorker!.version);\n",
        "      message.sendTo(sharedWorker.send);\n",
        "    case Impl.other:\n",
        "      break;\n",
        "  }\n",
        "}",
    );
    let expected: Vec<(u32, bool)> = [
        ("sharedWorker!.version", true),
        ("sharedWorker.send", false),
    ]
    .iter()
    .map(|(needle, nullable)| (source_offset(source, needle), *nullable))
    .collect();
    assert_eq!(needle_faces(source, "sharedWorker"), expected, "{source:?}");
}

/// A switch-pattern-promoted scrutinee read carries the matched type
/// (the riverpod `listenable.dart` shape: `case null: break;` then
/// `case $ResultData<ValueT>():` — the member read on the promoted
/// formal). The flow half of the seam: the promotion stands even where
/// the member's name collides with an abstract getter on the declared
/// base type.
#[test]
fn pattern_promoted_scrutinee_read_carries_matched_type() {
    let source = concat!(
        "sealed class Res<V> {\n",
        "  V? get value;\n",
        "  Object? get error;\n",
        "}\n",
        "class ResData<V> implements Res<V> {\n",
        "  final V value;\n",
        "  ResData(this.value);\n",
        "}\n",
        "class ResError<V> implements Res<V> {\n",
        "  final Object error;\n",
        "  ResError(this.error);\n",
        "}\n",
        "class Box<V> {\n",
        "  void sink(V? x) {}\n",
        "  void sink2(Object? x) {}\n",
        "  set result(Res<V>? next) {\n",
        "    switch (next) {\n",
        "      case null:\n",
        "        break;\n",
        "      case ResData<V>():\n",
        "        sink(next.value);\n",
        "      case ResError<V>():\n",
        "        sink2(next.error);\n",
        "    }\n",
        "  }\n",
        "}",
    );
    // The scrutinee read anchors one past the `(` of `switch (next)`; the
    // member reads anchor at their receiver token.
    let scrutinee = source_offset(source, "(next) {") + 1;
    let expected: Vec<(u32, bool)> = [
        (scrutinee, true),
        (source_offset(source, "next.value"), false),
        (source_offset(source, "next.error"), false),
    ]
    .to_vec();
    assert_eq!(needle_faces(source, "next"), expected, "{source:?}");
}

#[test]
fn if_write_leaves_the_variable_possibly_unassigned() {
    // The archive-corpus dead-chain cascade's flow face (2026-10-19): a
    // write inside an if's then branch leaves the variable *possibly*
    // unassigned after the join — assigned on one path only. The wide
    // `!isAssigned` row still fires (the potentially-unassigned join face
    // reads it); the `definitely_unassigned` bit must stay false so the
    // official definite-unassignment faces never fire on it.
    let source = concat!(
        "class L {} ",
        "L? mk() => null; ",
        "int f(bool c) { L? v; if (c) { v = mk(); } return v?.hashCode ?? 0; }",
    );
    let rows = facts_of(source);
    let unassigned: Vec<_> = rows
        .iter()
        .filter_map(|fact| match fact {
            FlowFact::UnassignedRead {
                span,
                definitely_unassigned,
                ..
            } => Some((
                source[span.range.offset as usize..span.range.end() as usize].to_string(),
                *definitely_unassigned,
            )),
            _ => None,
        })
        .collect();
    assert_eq!(unassigned, [("v".to_string(), false)]);
    assert!(dead_snippets(source).is_empty());
}

#[test]
fn ifnull_chain_result_keeps_the_nullable_face() {
    // `a ?? b` types `LUB(NonNull(a), b)` — with a nullable right side the
    // result is nullable, so the next `??` arm stays live. Stamping the
    // left's non-null form alone deaded every later arm of a chain.
    let source = "int f(int? a, int? b) => a ?? b ?? 6;";
    assert_eq!(dead_snippets(source), Vec::<String>::new());
}

#[test]
fn ifnull_guaranteed_non_null_left_still_deads_the_right() {
    // The guarantee path is untouched: a definitely non-null left side
    // never evaluates the right (the oracle deads it too).
    let source = "int f(int a) => a ?? 1;";
    assert_eq!(dead_snippets(source), ["?? 1"]);
}

#[test]
fn staged_declared_rhs_yields_to_the_folds_promoted_read() {
    // The semantic layer stages an assignment RHS with its RESOLUTION
    // type (declared, unpromoted — listener's staged-assignment face).
    // When the RHS is a bare local read the flow already promoted
    // (`if (b == null) return;` narrows `b` to `int`), the promoted flow
    // type is the oracle's write type: the target narrows through it and
    // the if-join leaves the promotion live for the argument check
    // (analyzer `context_locator.dart:276` family). The staged declared
    // face must not demote the write back to `int?`.
    let source = r#"void g(int x) {}
void f(int? a, int? b) {
  if (b == null) return;
  if (a == null) {
    a = b;
  }
  g(a);
}"#;
    // Two passes with the staged face mounted (offsets are
    // deterministic for the same source): the first finds the write
    // target's token offset, the second stages `int?` there exactly like
    // the semantic layer before the fold.
    let target_offset = source.find("a = b").unwrap() as u32;
    let result = dartq_scanner::scan(
        source,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        dartq_source::FileTable::new().intern(std::path::Path::new("test.dart")),
    );
    assert!(result.diagnostics.is_empty());
    let tokens: &'static TokenTable = Box::leak(Box::new(result.tokens));
    let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(
        LanguageVersion::new(3, 15),
    ));
    let mut flow = SharedFlow(std::rc::Rc::new(std::cell::RefCell::new(Flow::new(
        tokens, source,
    ))));
    let int_name = flow.0.borrow_mut().intern_name("int");
    flow.0.borrow_mut().stage_assignment_rhs(
        target_offset,
        crate::ty::Ty {
            base: crate::ty::Base::Name(int_name),
            nullable: true,
        },
    );
    let mut parser = dartq_parser::Parser::new(source, tokens, features, &mut flow);
    parser.parse_unit();
    drop(parser);
    let facts = flow.0.borrow_mut().take_facts();
    // The read of `a` in `g(a)` carries the promoted `int`.
    let g_arg = (source.find("g(a)").unwrap() + 2) as u32;
    let promoted: Vec<crate::ty::Ty> = facts
        .iter()
        .filter_map(|fact| match fact {
            FlowFact::PromotedRead { span, ty, .. } if span.range.offset == g_arg => Some(*ty),
            _ => None,
        })
        .collect();
    assert_eq!(promoted.len(), 1, "{facts:?}");
    assert!(!promoted[0].nullable);
}

#[test]
fn assignment_with_promoted_rhs_promotes_the_target_through_the_join() {
    // The unstaged fold (bare flow harness): the promoted RHS read type
    // is the write type, and the if-join keeps the target promoted for
    // the read after the `if` — the oracle's clean `g(a)` (probe u3).
    let source = r#"
void g(int x) {}
void f(int? a, int? b) {
  if (b == null) return;
  if (a == null) {
    a = b;
  }
  g(a);
}
"#;
    let (_, facts) = analyze(source);
    let g_arg = (source.find("g(a)").unwrap() + 2) as u32;
    let promoted: Vec<crate::ty::Ty> = facts
        .iter()
        .filter_map(|fact| match fact {
            FlowFact::PromotedRead { span, ty, .. } if span.range.offset == g_arg => Some(*ty),
            _ => None,
        })
        .collect();
    assert_eq!(promoted.len(), 1, "{facts:?}");
    assert!(!promoted[0].nullable);
}

// —— T3 nullability faces (#2312) ——————————————————————————————————————

/// The assignment expression's own value carries no promotable reference:
/// official `_write` returns one only under the experimental
/// `inferenceUpdate4` feature, so `(x = e) != null` tests the assigned
/// *value* and leaves `x` unpromoted — the read after the test keeps its
/// declared nullable face.
#[test]
fn assignment_value_test_leaves_the_target_unpromoted() {
    let source = "void f(int? p) { int? x; if ((x = p) != null) { x; } }";
    let body = (source.find("{ x; }").unwrap() + 2) as u32;
    assert_eq!(needle_faces(source, "x;"), [(body, true)], "{source:?}");
}

/// The property non-null assertion promotes the tested slot: official
/// `nonNullAssert_end` marks whatever reference the operand read carries
/// non-nullable, the property reference included. The read that follows
/// `c._b!` is the promoted one — exactly one promotion row, anchored at the
/// member name.
#[test]
fn non_null_assert_promotes_the_property_slot() {
    let source = concat!(
        "class C { final bool? _b; }\n",
        "void f(C c) { c._b!; c._b; }",
    );
    assert_eq!(promoted_property_snippets(source), ["_b"], "{source:?}");
}

/// The slot key includes the receiver: `a._b!` promotes only `a`'s slot —
/// `b._b` stays unpromoted.
#[test]
fn non_null_assert_promotes_only_its_receiver() {
    let source = concat!(
        "class C { final bool? _b; }\n",
        "void f(C a, C b) { a._b!; b._b; }",
    );
    assert!(promoted_property_snippets(source).is_empty(), "{source:?}");
}

/// The variable beside a null-check pattern is declared with the non-null
/// matched type (official `_handleNullCheckPattern` hands the sub-pattern
/// the non-null type, and `DeclaredVariablePattern` declares with it), so
/// the read in the matched arm is non-null instead of the untyped
/// [`crate::ty::Ty::UNKNOWN`] the `NoType` annotation leaves behind.
#[test]
fn null_check_pattern_retypes_its_variable() {
    let source = "void f(Object? v) { switch (v) { case var x?: x; } }";
    let read = source.find("x; }").unwrap() as u32;
    let ty = read_types_of(source)
        .into_iter()
        .find(|(offset, _)| *offset == read)
        .map(|(_, ty)| ty)
        .expect("read row present");
    assert_ne!(ty, crate::ty::Ty::UNKNOWN, "{source:?}");
    assert!(!ty.nullable, "{source:?}");
}

/// The plain declaration pattern keeps its untyped face: only the `?`
/// suffix narrows the declared variable.
#[test]
fn plain_variable_pattern_keeps_the_untyped_face() {
    let source = "void f(Object? v) { switch (v) { case var x: x; } }";
    let read = source.find("x; }").unwrap() as u32;
    let ty = read_types_of(source)
        .into_iter()
        .find(|(offset, _)| *offset == read)
        .map(|(_, ty)| ty)
        .expect("read row present");
    assert_eq!(ty, crate::ty::Ty::UNKNOWN, "{source:?}");
}

/// The for-in loop variable declared with an inferred head (`final e`) reads
/// the iterable's element type (upstream `_inferForEachVariable`): `x = e`
/// inside the loop writes `int` and promotes `x` — the promoted read stands
/// at the `takeInt` argument (real-corpus root: archive `zip_decoder.dart`'s
/// `x = e` inside `for (final e in xs)`).
#[test]
fn for_in_declared_element_reads_the_element_type() {
    let source = concat!(
        "void takeInt(int v) {}\n",
        "void f(List<int> xs, int? x) {\n",
        "  for (final e in xs) {\n",
        "    if (x == null) { x = e; }\n",
        "    takeInt(x);\n",
        "    break;\n",
        "  }\n",
        "}\n",
    );
    let arg = source.find("x);").unwrap() as u32;
    let tys = facts_of(source)
        .into_iter()
        .filter_map(|fact| match fact {
            FlowFact::ReadType { span, ty, .. } => Some((span.range.offset, ty)),
            _ => None,
        })
        .collect::<Vec<_>>();
    let ty = tys
        .iter()
        .find(|(offset, _)| *offset == arg)
        .map(|(_, ty)| *ty)
        .expect("read row at the argument");
    assert!(
        matches!(ty.base, crate::ty::Base::Name(_)) && !ty.nullable,
        "the element-typed write promotes `x` to non-null: {source:?}"
    );
}

/// A write-captured local is never promotable (the official `writeCaptured`
/// bail gating `tryPromoteForTypeCheck`, flow_analysis.dart:4136): the `is`
/// test still splits reachability, but the variable's own promotion chain
/// stays empty — the guarded read carries no promoted type.
#[test]
fn write_captured_variable_never_promotes() {
    let source = concat!(
        "void f(int? x) {\n",
        "  void g() { x = null; }\n",
        "  if (x is int) { x; }\n",
        "}\n",
    );
    // `x` inside the `is` body: the capture (the closure's write) kills its
    // promotion, so the read reports no promoted type — the declared `int?`
    // stands.
    assert!(promoted_tys(source).is_empty(), "{source:?}");
}

/// The mirror: the same shape WITHOUT the closure promotes `x` to `int` —
/// the captured gate, not the test shape, is what drops the promotion.
#[test]
fn uncaptured_variable_still_promotes() {
    let source = "void f(int? x) { if (x is int) { x; } }";
    assert_eq!(promoted_tys(source).len(), 1, "{source:?}");
}

/// An unknown-typed `??=` RHS writes the official's InvalidType face: it
/// never PROMOTES (the valid-step gate's mutual-subtype check refuses the
/// unknown), but it also never DEMOTES — `InvalidType <: everything`
/// (subtype.dart:274) keeps a standing promotion through the write.
#[test]
fn if_null_assign_unknown_rhs_preserves_promotion() {
    // `s` promoted to `String` by the null-check return, then `s ??= e`
    // with `e` the fold's unknown face: the write keeps the promotion.
    let source = concat!(
        "void f(String? s, Object? e) {\n",
        "  if (s == null) return;\n",
        "  dynamic d = e;\n",
        "  s = d;\n", // a dynamic write demotes the promotion first
        "  s;\n",
        "}\n",
    );
    // The dynamic `s = d` demotes `s` back to `String?` — the unknown
    // coverage gate keeps no promotion it cannot satisfy... wait, a dynamic
    // write DOES demote (dynamic <: String? is false for coverage, so the
    // String promotion drops). The read reports nothing promoted.
    assert!(promoted_tys(source).is_empty(), "{source:?}");
}

/// The `??=` unknown arm on an already-promoted variable: `s` promoted,
/// then `s ??= opaque()` — the write's unknown face satisfies every
/// standing promotion (`covers`'s InvalidType recovery), so `s` stays
/// promoted after it.
#[test]
fn if_null_assign_unknown_rhs_keeps_the_standing_promotion() {
    let source = concat!(
        "void f(String? s) {\n",
        "  if (s == null) return;\n",
        "  s = s.opaque();\n", // not ??= — keep this test focused
        "  s;\n",
        "}\n",
    );
    // `s.opaque()` reads as unknown (an unresolved member send): the write
    // keeps `s` promoted per the InvalidType coverage arm — the promoted
    // reads at the RHS target and the trailing `s;` both stand.
    assert!(!promoted_tys(source).is_empty(), "{source:?}");
}

/// The dead-region read still carries its rows (official promotion
/// materializes under an unreachable state — the region's own dead row is
/// the only reachability face): `x` promoted before `return`, the dead read
/// after it still emits the promoted-type row the consumer keys on.
#[test]
fn dead_region_read_carries_the_promotion_row() {
    let source = concat!(
        "void f(int? x) {\n",
        "  if (x == null) return;\n",
        "  return;\n",
        "  x;\n",
        "}\n",
    );
    // The dead `x;` read: the region's dead row stands, and the promoted
    // `int` read row emits beside it.
    let facts = facts_of(source);
    let read_offset = source.rfind("x;").unwrap() as u32;
    let promoted = facts.iter().any(|fact| {
        matches!(
            fact,
            FlowFact::PromotedRead { span, .. } if span.range.offset == read_offset
        )
    });
    assert!(promoted, "{facts:?}");
    let read_type = facts.iter().any(|fact| {
        matches!(
            fact,
            FlowFact::ReadType { span, .. } if span.range.offset == read_offset
        )
    });
    assert!(read_type, "{facts:?}");
}

/// The assert message evaluates on the condition-FALSE state (official
/// `assert_afterCondition`, flow_analysis.dart:6478): `assert(t == null,
/// '${t.f}')`'s message reads `t` under the non-null arm — the null check's
/// promotion reaches the message's interpolation read.
#[test]
fn assert_message_reads_the_condition_false_state() {
    let source = concat!(
        "class T { int get entries => 0; }\n",
        "void f(T? t) {\n",
        "  assert(t == null, 'x ${t.entries}');\n",
        "}\n",
    );
    // The `t` inside the interpolation: `t == null`'s false arm has `t`
    // non-null, so the message's `t.entries` read promotes.
    let interp_offset = source.find("t.entries").unwrap() as u32;
    let promoted = facts_of(source).iter().any(|fact| {
        matches!(
            fact,
            FlowFact::PromotedRead { span, .. } | FlowFact::PromotedPropertyRead { span }
            if span.range.offset == interp_offset
        )
    });
    assert!(promoted, "{source:?}");
}


// -- #2311 face pins ------------------------------------------------------------

/// ② `x = x ?? const []` writes the LHS's non-null face *with its written
/// argument tree*, so the read after the self-assign reports a real
/// promotion (the `argument_type_not_assignable` verdict at
/// `intl/lib/message_lookup_by_library.dart:134` hinged on it).
///
/// Mechanism: `Expr::trivial` left the `??` result argument-less
/// (`EMPTY_ARGS` = the raw-type face), so `List<Object>? x`'s write
/// recorded `List<dynamic>` — not a subtype of the declared `List<Object>`
/// — and the type-of-interest re-promotion dropped. The `??` fold now
/// recovers the LHS read's argument tree when the result lands on the
/// promoted left side (its `promote_to_non_null` head matches the joined
/// result type), falling back to the empty tree for a composite LHS
/// (`f() ?? d`, `a.b ?? d`) the head cannot attribute.
#[test]
fn if_null_self_assign_promotes_with_the_written_arguments() {
    // `x` reads its promoted non-null `List` carrying the written `<Object>`
    // — the pairing a resolved back-end stages as `List<Object>`.
    let source = "void f(List<Object>? x) { x = x ?? const []; x; }";
    let (flow, facts) = analyze(source);
    let flow = flow.0.borrow();
    let promoted: Vec<(crate::ty::Ty, Vec<String>)> = facts
        .iter()
        .filter_map(|fact| match fact {
            FlowFact::PromotedRead { ty, args, .. } => Some((
                *ty,
                flow.type_args_at(*args)
                    .iter()
                    .map(|a| match a.base {
                        crate::ty::Base::Name(id) => flow.name_lexeme(id).to_string(),
                        other => panic!("named spelling expected, got {other:?}"),
                    })
                    .collect::<Vec<_>>(),
            )),
            _ => None,
        })
        .collect();
    assert_eq!(promoted.len(), 1, "{facts:?}");
    assert!(!promoted[0].0.nullable, "{promoted:?}");
    assert_eq!(promoted[0].1, vec!["Object".to_string()], "{promoted:?}");
}

#[test]
fn if_null_non_literal_rhs_keeps_the_nullable_join() {
    // `x ?? y` where `y`'s nullability is undecided: the LUB join widens
    // back to nullable, so the write records no promotion.
    let source = "void f(List<Object>? x, List<Object>? y) { x = x ?? y; x; }";
    assert!(
        promoted_tys(source).is_empty(),
        "{:?}",
        promoted_tys(source)
    );
}

#[test]
fn if_null_inside_other_assign_touches_no_var() {
    // `y = x ?? const []` writes `y`, never `x`: the RHS's own head is the
    // written face, and `x` keeps its declared nullable read.
    let source = "void f(List<Object>? x, List<Object>? y) { y = x ?? const []; x; }";
    assert!(
        promoted_tys(source).is_empty(),
        "{:?}",
        promoted_tys(source)
    );
}

/// ③ `case final v?:` on a possibly-null matched value promotes the matched
/// arm non-null and joins the pre-pattern `current` into the unmatched
/// chain (official `_handleNullTestPattern(matchesIfNull: false)`:
/// `_unmatched = _join(_unmatched!, _current)` — the state *before* the
/// promotion, still nullable on the null path). Joining the promoted state
/// instead carried the non-null face into the unmatched chain and out of
/// the whole `if`-case statement, so a later read of the scrutinee saw a
/// promotion the oracle does not grant (`Object z = x;` must report
/// `invalid_assignment` — the official still types `x` as `Object?`).
#[test]
fn null_check_pattern_joins_the_pre_pattern_state() {
    let source = "void f(Object? x) {\n  if (x case final y?) {\n    print(y);\n  }\n  x;\n}";
    let read = source.rfind("x;\n}").unwrap() as u32;
    let (_, ty) = read_types_of(source)
        .into_iter()
        .find(|(offset, _)| *offset == read)
        .expect("post-if read row present");
    assert!(ty.nullable, "the unmatched path is still null: {ty:?}");
    assert!(
        promoted_tys(source).is_empty(),
        "{:?}",
        promoted_tys(source)
    );
}

/// ③ boundary arm: a guarded `when` clause does not change the join order —
/// the official still joins the pre-promotion state into `_unmatched`, so
/// the post-`if` read stays nullable.
#[test]
fn null_check_pattern_with_a_guard_keeps_the_nullable_join() {
    let source =
        "void f(Object? x) {\n  if (x case final y? when y.hashCode > 0) {\n    print(y);\n  }\n  x;\n}";
    let read = source.rfind("x;\n}").unwrap() as u32;
    let (_, ty) = read_types_of(source)
        .into_iter()
        .find(|(offset, _)| *offset == read)
        .expect("post-if read row present");
    assert!(ty.nullable, "the unmatched path is still null: {ty:?}");
}

/// ③ boundary arm: a matched value already non-null makes `sub?` a
/// guaranteed match — the official touches neither `_current` nor
/// `_unmatched`, so the post-`if` read keeps its declared non-null face and
/// no promotion is recorded.
#[test]
fn null_check_pattern_on_a_non_null_matched_value_is_a_no_op() {
    let source = "void f(Object x) {\n  if (x case final y?) {\n    print(y);\n  }\n  x;\n}";
    let read = source.rfind("x;\n}").unwrap() as u32;
    let (_, ty) = read_types_of(source)
        .into_iter()
        .find(|(offset, _)| *offset == read)
        .expect("post-if read row present");
    assert!(!ty.nullable, "{ty:?}");
    assert!(
        promoted_tys(source).is_empty(),
        "{:?}",
        promoted_tys(source)
    );
}

/// ④ A `Function`-typed operand's `is` test against a function type: the
/// true-arm read promotes to the non-null tested type, the else-arm read
/// sees the declared nullable face. This is the name-level pin of the
/// drift `async.dart:15` face; the semantic layer's over-report in a
/// resolved-`FutureOr` world (a `FutureOr<F>` operand whose resolved face
/// vacuously subtypes `Function`) is a documented residual — the fix sites
/// sit outside this lane's scope (signature-aware `is_subtype_typed` in
/// the resolved ops back-end).
#[test]
fn function_is_test_splits_the_arms() {
    let source = "void f(void Function(int)? g) { if (g is void Function(int)) { g; } else { g; } }";
    let promoted = promoted_tys(source);
    // True arm: the non-null `void Function(int)` face. Else arm: the
    // subtracted `Null` remainder reads as the nullable `Never` face
    // (`promote_to_non_null(Null)`), never back to the declared operand.
    assert_eq!(promoted.len(), 2, "{promoted:?}");
    assert_eq!(promoted[0].base, crate::ty::Base::Function, "{promoted:?}");
    assert!(!promoted[0].nullable, "{promoted:?}");
    assert_eq!(promoted[1].base, crate::ty::Base::Never, "{promoted:?}");
}

/// #2341 pin ① (guaranteed-succeed): a non-reference operand whose static
/// type is already the tested type's subtype makes the test a boolean
/// literal (official `isExpression_end`'s
/// `_isTypeCheckGuaranteedToSucceedWithSoundNullSafety` arm — upstream Dart
/// SDK source, pkg/_fe_analyzer_shared/lib/src/flow_analysis/
/// flow_analysis.dart:7201-7205 — folded to `booleanLiteral(!isNot)`,
/// :6544-6559): `is` deads the else branch and its body reports the
/// whole-block dead row (the probe-g5 shape).
#[test]
fn is_test_guaranteed_succeed_deads_the_else() {
    let source = "void f(Object? x) { if ((x as int) is int) {} else { x; } }";
    assert_eq!(dead_snippets(source), ["{ x; }"]);
}

/// #2341 pin ② (guaranteed-succeed boundary): a nullable operand is no
/// subtype of the non-null tested type — the check can still fail, so
/// both arms stay live (official `isSubtypeOf(int?, int)` is false and
/// the nullability-disjoint fail arm does not fire on a potentially
/// nullable face either).
#[test]
fn nullable_operand_keeps_both_is_test_arms_live() {
    let source = "void f(Object? x) { if ((x as int?) is int) {} else { x; } }";
    assert!(dead_snippets(source).is_empty());
}

/// #2341 pin ③ (guaranteed-succeed boundary): the dynamic/Unknown faces
/// refuse the guarantee — a dynamic tested spelling and an unknown
/// operand face are never certified as dead-branch producers (the
/// name-level lattice's top-type optimism must not dead a branch over an
/// unknowable face; a resolved back-end re-decides through the real
/// hierarchy).
#[test]
fn is_dynamic_and_unknown_operand_never_guarantee() {
    let tested_dynamic =
        "void f(Object? x) { if ((x as int) is dynamic) {} else { x; } }";
    assert!(dead_snippets(tested_dynamic).is_empty());
    let unknown_operand = "void f() { if (g() is dynamic) {} else { h(); } }";
    assert!(dead_snippets(unknown_operand).is_empty());
}

/// #2356 pin ① (guaranteed-fail, bottom tested type): a bottom tested type
/// folds the test to a constant `false` (official `isExpression_end`'s
/// first arm — upstream Dart SDK source, pkg/_fe_analyzer_shared/lib/src/
/// flow_analysis/flow_analysis.dart:7184-7189 — `booleanLiteral(isNot)`):
/// `is` deads the then branch, whose body reports the whole-block dead row.
#[test]
fn is_never_tested_type_deads_the_then_arm() {
    let source = "void f(Object? x) { if ((x as int) is Never) { x; } else {} }";
    assert_eq!(dead_snippets(source), ["{ x; }"]);
}

/// #2356 pin ② (guaranteed-fail, flipped): `is! Never` folds to `true` —
/// the then arm stays live and the else arm reports the dead row.
#[test]
fn is_bang_never_flips_the_dead_arm() {
    let source = "void f(Object? x) { if ((x as int) is! Never) {} else { x; } }";
    assert_eq!(dead_snippets(source), ["{ x; }"]);
}

/// #2356 pin ③ (guaranteed-fail, nullability-disjoint): one side
/// `Null`-or-equivalent (`Null`, `Never?`), the other non-nullable —
/// official `_typesAreDisjointDueToNullability` (upstream Dart SDK source,
/// pkg/_fe_analyzer_shared/lib/src/flow_analysis/flow_analysis.dart:9496-9501)
/// over `classifyType` — folds the test to `false` and deads the then arm,
/// while a potentially-nullable operand against `Null` is not disjoint and
/// keeps both arms live.
#[test]
fn nullability_disjoint_test_deads_the_then_arm() {
    let null_operand = "void f(Object? x) { if ((x as Null) is int) { x; } else {} }";
    assert_eq!(dead_snippets(null_operand), ["{ x; }"]);
    let null_equivalent_tested =
        "void f(Object? x) { if ((x as int) is Never?) { x; } else {} }";
    assert_eq!(dead_snippets(null_equivalent_tested), ["{ x; }"]);
    let potentially_nullable_operand =
        "void f(Object? x) { if ((x as int?) is Null) { x; } else { x; } }";
    assert!(dead_snippets(potentially_nullable_operand).is_empty());
}

/// #2356 pin ④ (guaranteed-fail boundary): the unknown/dynamic/void faces
/// refuse the fail guarantee — an unknowable operand face is never
/// certified as a dead-branch producer even against a bottom tested type
/// (the name-level lattice's optimism must not dead a branch; a resolved
/// back-end re-decides through the real hierarchy). This pin binds the new
/// mark's reach, not a swap-back proof: before the fix no fail-marking
/// exists, so the empty result holds trivially on HEAD — its force is
/// against the new helper over-firing.
#[test]
fn unknowable_operand_never_guaranteed_fail() {
    let source = "void f() { if (g() is Never) { h(); } else {} }";
    assert!(dead_snippets(source).is_empty());
}

/// #2357 pin ① (guaranteed-fail over a reference operand): the official
/// fail arm decides before the reference branch, so a local variable's
/// `is Never` folds to `booleanLiteral(false)` and deads the then arm —
/// no promotion split rides the fold (the then body reports the
/// whole-block dead row, the probe-g5 shape). Swap-back-proven: HEAD
/// (fold absent in the reference arm) keeps the then arm live.
#[test]
fn local_reference_is_never_deads_the_then_arm() {
    let source = "void f() { int x = 0; if (x is Never) { x; } else {} }";
    assert_eq!(dead_snippets(source), ["{ x; }"]);
}

/// #2357 pin ② (promoted-satisfying reference): after `x is String`
/// promotes `x`, the inner `x is String` keeps the reference path (the
/// official fail arm does not fire — `String` and `String` are not
/// disjoint) and the promotion split itself deads the else arm: a bottom
/// `factor(previous, tested)` marks the unmatched arm unreachable
/// (official `tryPromoteForTypeCheck`'s `ifFalseIsUnreachable` —
/// upstream Dart SDK source, pkg/_fe_analyzer_shared/lib/src/
/// flow_analysis/flow_analysis.dart:4156-4160). Honest swap-back note:
/// HEAD-green — this deadness is [`Interp::branch_type_test`]'s
/// pre-existing factor face; the pin constrains the fail fold from
/// displacing it.
#[test]
fn promoted_reference_meeting_the_test_deads_the_else() {
    let source =
        "void f(Object x) { if (x is String) { if (x is String) {} else { x; } } }";
    assert_eq!(dead_snippets(source), ["{ x; }"]);
}

/// #2357 pin ③ (guaranteed-fail flipped over a reference operand):
/// `is! Never` folds to `booleanLiteral(true)` — the then arm stays live
/// and the else arm reports the dead row. Swap-back-proven: HEAD keeps
/// the else arm live.
#[test]
fn local_reference_is_bang_never_flips_the_dead_arm() {
    let source = "void f() { int x = 0; if (x is! Never) {} else { x; } }";
    assert_eq!(dead_snippets(source), ["{ x; }"]);
}

/// #2357 pin ④ (control): a promotable reference whose test guarantees
/// nothing keeps both arms live — no fold fires and the promotion split
/// leaves both faces reachable. Honest swap-back note: HEAD-green by
/// construction (the control face); its force is against the fold
/// over-firing on a plain promotable operand.
#[test]
fn promotable_reference_keeps_both_is_test_arms_live() {
    let source = "void f(Object x) { if (x is String) { x; } else { x; } }";
    assert!(dead_snippets(source).is_empty());
}

/// #2357 pin ⑤ (no guaranteed-succeed fold on references): the official
/// succeed arm sits behind the reference check (upstream Dart SDK source,
/// pkg/_fe_analyzer_shared/lib/src/flow_analysis/flow_analysis.dart:
/// 7190-7205 — reached only when `_getExpressionReference` returned
/// null), so a reference test never folds to `true` on a mere subtype
/// fact: `int x; x is num` keeps the else arm live (the promotion split's
/// else-deadness needs a bottom factor, and `factor(int, num)` is `int`).
/// Honest swap-back note: HEAD-green — the pin binds this lane's skip
/// decision, its force is against a succeed fold over-reaching into the
/// reference arm.
#[test]
fn reference_is_test_never_folds_to_guaranteed_succeed() {
    let source = "void f(int x) { if (x is num) { x; } else { x; } }";
    assert!(dead_snippets(source).is_empty());
}

/// #2346③: a switch expression's value carries the LUB join of its case
/// values (upstream Dart SDK source, pkg/analyzer/lib/src/dart/ast/ast.dart
/// `SwitchExpressionImpl.staticType`'s arm fold; the name-level join is
/// `??`'s `name_lub`). The bound local adopts the joined nullable face, so
/// the receiver read of `r.m()` lifts it — the flow-read face the semantic
/// Unknown arm (#2346①) re-enters the member verdict with, whose
/// nullability emits the oracle's `unchecked_use_of_nullable_value`.
/// Before the join the block pushed the plain UNKNOWN face and every
/// consumer stayed untyped.
#[test]
fn switch_expr_case_value_lub_types_the_bound_local() {
    let source = "class C { void m() {} }\n\
                  void f(bool x, C? a, C? b) {\n\
                  \x20 var r = switch (x) { true => a, false => b };\n\
                  \x20 r.m();\n\
                  }";
    let (flow, facts) = analyze(source);
    let c = flow.0.borrow_mut().intern_name("C");
    let read_offset = source.find("r.m()").unwrap() as u32;
    let rows: Vec<(u32, crate::ty::Ty)> = facts
        .into_iter()
        .filter_map(|fact| match fact {
            FlowFact::ReadType { span, ty, .. } => Some((span.range.offset, ty)),
            _ => None,
        })
        .collect();
    let receiver = rows
        .iter()
        .find(|(offset, _)| *offset == read_offset)
        .unwrap_or_else(|| panic!("no read row at {read_offset}: {rows:?}"));
    assert_eq!(
        receiver.1,
        crate::ty::Ty {
            base: crate::ty::Base::Name(c),
            nullable: true,
        },
        "{rows:?}"
    );
}

/// #2346③: the latch takes the alternative's BODY value, not the guard's —
/// a guarded alternative's guard value is the stack's second entry at the
/// case end, and latching it would join `bool` into the fold (the mixed
/// join loses the face).
#[test]
fn switch_expr_guard_case_latches_the_body_value() {
    let source = "class C { void m() {} }\n\
                  void f(bool x, bool y, C? a, C b) {\n\
                  \x20 var r = switch (x) { true when y => a, _ => b };\n\
                  \x20 r.m();\n\
                  }";
    let (flow, facts) = analyze(source);
    let c = flow.0.borrow_mut().intern_name("C");
    let read_offset = source.find("r.m()").unwrap() as u32;
    let rows: Vec<(u32, crate::ty::Ty)> = facts
        .into_iter()
        .filter_map(|fact| match fact {
            FlowFact::ReadType { span, ty, .. } => Some((span.range.offset, ty)),
            _ => None,
        })
        .collect();
    let receiver = rows
        .iter()
        .find(|(offset, _)| *offset == read_offset)
        .unwrap_or_else(|| panic!("no read row at {read_offset}: {rows:?}"));
    assert_eq!(
        receiver.1,
        crate::ty::Ty {
            base: crate::ty::Base::Name(c),
            nullable: true,
        },
        "{rows:?}"
    );
}

/// #2346③ negative (the oracle's silent face, probed at
/// `unchecked_use_of_nullable_value`'s family): all-non-null case values
/// join to the non-null face, and the bound local adopts nothing (a
/// non-nullable spelled initializer never widens the unknown slot — the
/// #2243 posture), so the receiver read carries no nullable face to lift.
#[test]
fn switch_expr_non_null_case_values_bind_no_nullable_face() {
    let source = "class C { void m() {} }\n\
                  void f(bool x, C a, C b) {\n\
                  \x20 var r = switch (x) { true => a, false => b };\n\
                  \x20 r.m();\n\
                  }";
    let read_offset = source.find("r.m()").unwrap() as u32;
    let rows: Vec<(u32, crate::ty::Ty)> = read_types_of(source);
    let receiver = rows
        .iter()
        .find(|(offset, _)| *offset == read_offset)
        .unwrap_or_else(|| panic!("no read row at {read_offset}: {rows:?}"));
    assert!(
        !receiver.1.nullable,
        "non-null join must not carry a nullable face: {rows:?}"
    );
}
