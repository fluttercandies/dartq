//! Unit tests mined from the official flow-analysis corpus
//! (`.third/dart-sdk/pkg/_fe_analyzer_shared/test/flow_analysis/`),
//! each citing its source file. Assertions cover the crate's fact face:
//! dead code and definite-unassigned reads.

use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_scanner::{ScanConfig, TokenTable};

use crate::facts::FlowFact;
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
    let flow = SharedFlow(std::rc::Rc::new(std::cell::RefCell::new(Flow::new(
        tokens, source,
    ))));
    let mut parser = dartq_parser::Parser::new(
        source,
        tokens,
        features,
        SharedFlow(std::rc::Rc::clone(&flow.0)),
    );
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
    let flow = SharedFlow(std::rc::Rc::new(std::cell::RefCell::new(Flow::new(
        tokens, source,
    ))));
    let mut parser = dartq_parser::Parser::new(
        source,
        tokens,
        features,
        SharedFlow(std::rc::Rc::clone(&flow.0)),
    );
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
            | FlowFact::BodyMayCompleteNormally { .. } => None,
        })
        .collect()
}

/// The read-face facts of a source (dead code, unassigned reads,
/// promoted reads) without the body-completion rows: the assertion face
/// of the tests that pin those three faces. A reachable executable
/// emits a completion row by design (#676), so a whole-list emptiness
/// check would observe it.
fn read_facts_of(source: &str) -> Vec<FlowFact> {
    facts_of(source)
        .into_iter()
        .filter(|fact| !matches!(fact, FlowFact::BodyMayCompleteNormally { .. }))
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
        had_type_args: false,
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
        crate::ev::Ev::InitializedIdent { name },
        crate::ev::Ev::InitializerBegin,
        crate::ev::Ev::OtherExpr { span: tspan(1) },
        crate::ev::Ev::InitializerEnd,
        crate::ev::Ev::VarDeclEnd,
    ]
}

/// Drives a hand-built buffer through a fresh interpreter (name-level ops).
fn interp_of(events: &[crate::ev::Ev]) -> crate::interp::Interp<'static> {
    let mut interp = crate::interp::Interp::new(&crate::ty::NameTypeOps);
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

// —— W3 dead-code oracle faces (#868, docs/audits/w3-deadcode-probes.md) ——

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
    // 18, `1;` at 20-21. The recorded grammar per expression statement:
    // `ExprStmt` + `StmtEnd` at the statement's begin, then the body
    // expression's events (record.rs `handle_expression_statement`); the
    // engine appends Exit after resolving the call's type (official call
    // site fires with the expression still on the stack).
    let events: Vec<crate::ev::Ev> = [
        vec![
            crate::ev::Ev::ExecBegin { is_closure: false },
            crate::ev::Ev::BlockBodyBegin,
        ],
        // `never();` — the call's type resolves to Never.
        vec![
            crate::ev::Ev::ExprStmt { span: tspan(11) },
            crate::ev::Ev::StmtEnd { end: 19 },
            crate::ev::Ev::OtherExpr { span: tspan(11) },
            crate::ev::Ev::Exit { end: tspan(18) },
        ],
        // `1;` — unreachable from here.
        vec![
            crate::ev::Ev::ExprStmt { span: tspan(20) },
            crate::ev::Ev::StmtEnd { end: 22 },
            crate::ev::Ev::OtherExpr { span: tspan(20) },
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
/// outer while's `flowEnd(node.body)` (type_analyzer.dart:4901) trimmed
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

/// official reachability/data/switch.dart `case_never_completes`: dead
/// code inside a case body whose both branches return.
#[test]
fn switch_case_inner_dead_code() {
    let source = "void f(bool b, int i) { switch (i) { case 1: 1; if (b) { return; } else { return; } 2; } 3; }";
    assert_eq!(dead_snippets(source), ["2;"]);
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

/// (c) The #611 suppression gate: a promotion whose tested type spelled
/// type arguments presents as a bare name at this seam, so its reads stay
/// out of the fact face; the arg-free twin (`x is List`) and nullability
/// promotions stay in scope.
#[test]
fn arg_bearing_promotions_suppressed_arg_free_emitted() {
    // `x is List<int>`: real promotion, erased arguments — suppressed.
    let arg_bearing = "void f(List? x) { if (x is List<int>) { x; } }";
    assert!(read_facts_of(arg_bearing).is_empty(), "{arg_bearing}");
    // `x is List`: the same promotion shape, no arguments — emitted.
    let arg_free = "void f(List? x) { if (x is List) { x; } }";
    let promoted = promoted_tys(arg_free);
    assert_eq!(promoted.len(), 1);
    assert!(matches!(promoted[0].base, crate::ty::Base::Name(_)));
    assert!(!promoted[0].nullable);
    // A multi-argument tested type suppresses too (same latch).
    let multi = "void f(Map? x) { if (x is Map<String, int>) { x; } }";
    assert!(read_facts_of(multi).is_empty());
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
/// (the sort arm covers all four fact kinds).
#[test]
fn take_facts_stays_offset_sorted_with_promotion_reads() {
    let source = "void f() { return; 1; } void g(num x) { if (x is int) { x; } }";
    let facts = facts_of(source);
    // f ends unreachable (no completion row); g falls through — its row
    // anchors at the unit's last token, so it sorts last.
    assert_eq!(facts.len(), 3, "{facts:?}");
    assert!(matches!(facts[0], FlowFact::DeadCode { .. }));
    assert!(matches!(facts[1], FlowFact::PromotedRead { .. }));
    assert!(matches!(facts[2], FlowFact::BodyMayCompleteNormally { .. }));
    let offsets: Vec<u32> = facts
        .iter()
        .map(|fact| match fact {
            FlowFact::DeadCode { span }
            | FlowFact::UnassignedRead { span, .. }
            | FlowFact::PromotedRead { span, .. }
            | FlowFact::BodyMayCompleteNormally { span } => span.range.offset,
        })
        .collect();
    assert!(offsets.windows(2).all(|pair| pair[0] <= pair[1]));
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
/// the enclosing body (the official gate fires per FunctionBody).
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
    let a_mid = lists.cons(t5, NIL);
    let a_tail = lists.cons(t3, a_mid);
    let a = lists.cons(t1, a_tail);
    let b_mid = lists.cons(t4, NIL);
    let b_tail = lists.cons(t3, b_mid);
    let b = lists.cons(t2, b_tail);
    let state = |promoted| {
        let mut s = State::new(0);
        s.models.push(VarModel {
            promoted,
            tested: NIL,
            version: 1,
            assigned: true,
            unassigned: false,
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
    let mut interp = Interp::new(&NameTypeOps);
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
        Ev::InitializedIdent { name: 0 },
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
    let mut interp = Interp::new(&NameTypeOps);
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
    let mut interp = Interp::new(&NameTypeOps);
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
    let mut interp = Interp::new(&NameTypeOps);
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
    let mut interp = Interp::new(&NameTypeOps);
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
    let mut interp = Interp::new(&NameTypeOps);
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
    let mut interp = Interp::new(&NameTypeOps);
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
    let mut interp = Interp::new(&NameTypeOps);
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
    let mut interp = Interp::new(&NameTypeOps);
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
    let mut interp = Interp::new(&NameTypeOps);
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
    let mut interp = Interp::new(&NameTypeOps);
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
/// Invariant-level: stack drains, no facts, replay stays reachable
/// (official `switch_beginAlternative`/`switch_endAlternative`,
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
        Ev::SwitchExprCaseEnd { has_guard: true },
        // `_ => e`.
        Ev::SwitchExprCaseBegin,
        Ev::PatternBegin,
        Ev::Wildcard { token: span(5) },
        Ev::PatternEnd,
        Ev::SwitchExprCasePattern,
        Ev::OtherExpr { span: span(6) },
        Ev::SwitchExprCaseEnd { has_guard: false },
        Ev::SwitchExprBlockEnd,
    ];
    let mut interp = Interp::new(&NameTypeOps);
    interp.run(&events);
    assert!(interp.stack.is_empty());
    assert!(interp.facts.is_empty());
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
    let mut interp = Interp::new(&NameTypeOps);
    interp.run(&events);
    assert!(interp.stack.is_empty());
    assert_eq!(interp.facts.len(), 1);
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
    let mut interp = Interp::new(&NameTypeOps);
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
        Ev::InitializedIdent { name: 0 },
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
        Ev::Assign { plain: true },
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
    let mut interp = Interp::new(&NameTypeOps);
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
        Ev::InitializedIdent { name: 0 },
        Ev::NoInitializer,
        Ev::VarDeclEnd,
        Ev::VarDeclBegin {
            late: false,
            is_final: false,
            span: span(2),
        },
        Ev::InitializedIdent { name: 1 },
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
    let mut interp = Interp::new(&NameTypeOps);
    interp.run(&events);
    assert!(interp.stack.is_empty());
    assert!(interp.facts.is_empty());
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
        },
        Ev::DeclaredVarPattern {
            name: 1,
            is_final: false,
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
    let mut interp = Interp::new(&NameTypeOps);
    interp.run(&events);
    assert!(interp.stack.is_empty());
    assert!(interp.facts.is_empty());
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
        Ev::InitializedIdent { name: 0 },
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
        Ev::AwaitBegin,
        Ev::Ident {
            name: 1,
            token: span(5),
            context: IdentifierContext::Expression,
        },
        Ev::SendNoArgs,
        Ev::AwaitEnd,
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
        Ev::Assign { plain: true },
        Ev::ExprStmt { span: span(8) },
    ];
    let mut interp = Interp::new(&NameTypeOps);
    interp.run(&events);
    assert!(interp.stack.is_empty());
    // The closure body falls off its end: one completion row.
    assert!(matches!(
        interp.facts.as_slice(),
        [FlowFact::BodyMayCompleteNormally { .. }]
    ));
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
        Ev::InitializedIdent { name: 0 },
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
        Ev::Assign { plain: true },
        Ev::ExprStmt { span: span(5) },
        Ev::ForInBodyEnd,
        Ev::ForInEnd,
    ];
    let mut interp = Interp::new(&NameTypeOps);
    interp.run(&events);
    assert!(interp.stack.is_empty());
    assert!(interp.exprs.is_empty());
    assert!(interp.facts.is_empty());
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
        Ev::InitializedIdent { name: 0 },
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
    let mut interp = Interp::new(&NameTypeOps);
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
        Ev::InitializedIdent { name: 0 },
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
    let mut interp = Interp::new(&NameTypeOps);
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
/// dead_code_verifier.dart:253 — the AST node starts at `L`), so the
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
/// `visitForStatement`'s `flowEnd(node.body)` (type_analyzer.dart:3194);
/// the body block is EMPTY, so the trim's `statements.isNotEmpty` guard
/// keeps the block itself (dead_code_verifier.dart:269-271) and the row
/// ends at its `}`.
#[test]
fn dead_interval_for_empty_body_runs_to_the_brace() {
    let source = "void f() { return; for (;;) {} }";
    assert_eq!(dead_snippets(source), ["for (;;) {}"]);
}

/// `visitWhileStatement`'s `flowEnd(node.body)` (type_analyzer.dart:4901)
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
/// `firstDead.firstToken` reads the AST node (dead_code_verifier.dart
/// :253/:325-332), while fasta's `beginLabeledStatement(next, …)`
/// (parser_impl.dart:6909) delivers the statement PAST the labels as the
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
/// dead loop anchors at `for` itself (`for_begin`, official ForStatement
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
        // `boom();` — the call's type resolves to Never.
        vec![
            crate::ev::Ev::ExprStmt { span: tspan(11) },
            crate::ev::Ev::StmtEnd { end: 18 },
            crate::ev::Ev::OtherExpr { span: tspan(11) },
            crate::ev::Ev::Exit { end: tspan(18) },
        ],
        // `1;` — unreachable from here.
        vec![
            crate::ev::Ev::ExprStmt { span: tspan(19) },
            crate::ev::Ev::StmtEnd { end: 21 },
            crate::ev::Ev::OtherExpr { span: tspan(19) },
        ],
        // `if (c) 2;` — folds into the same interval.
        vec![
            crate::ev::Ev::IfBegin { span: tspan(22) },
            crate::ev::Ev::OtherExpr { span: tspan(26) },
            crate::ev::Ev::ThenBegin,
            crate::ev::Ev::ExprStmt { span: tspan(29) },
            crate::ev::Ev::StmtEnd { end: 31 },
            crate::ev::Ev::OtherExpr { span: tspan(29) },
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
        // `g(boom());` — the argument's type resolves to Never.
        vec![
            crate::ev::Ev::ExprStmt { span: tspan(11) },
            crate::ev::Ev::StmtEnd { end: 21 },
            crate::ev::Ev::OtherExpr { span: tspan(11) },
            crate::ev::Ev::Send,
            crate::ev::Ev::ArgumentsBegin,
            crate::ev::Ev::OtherExpr { span: tspan(13) },
            crate::ev::Ev::Exit { end: tspan(19) },
            crate::ev::Ev::ArgumentsEnd { count: 1 },
        ],
        // `1;` — unreachable from here.
        vec![
            crate::ev::Ev::ExprStmt { span: tspan(22) },
            crate::ev::Ev::StmtEnd { end: 24 },
            crate::ev::Ev::OtherExpr { span: tspan(22) },
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
            Ev::Assign { plain: true },
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
/// the else branch is dead code.
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
                FlowFact::DeadCode { .. },
                FlowFact::BodyMayCompleteNormally { .. }
            ]
        ),
        "the else branch of `x is int?` on `int x` is provably dead: {:?}",
        interp.facts
    );
}

/// `??` types as the LHS's non-null form (official `ifNullExpression_end`,
/// flow_analysis.dart:7071-7080, with the skip-path promotion mounted at
/// `ifNullExpression_rightBegin`, :7086-7104). The old engine gave `??`
/// no split and a typeless result.
#[test]
fn if_null_types_as_the_lhs_non_null_form() {
    use crate::ev::Ev;
    use crate::ty::{Base, Ty};
    const INT: u32 = 102;
    const X: u32 = 7;
    let events: Vec<Ev> = [
        declared(X, named_ty(INT, true)),
        vec![
            read(X),
            Ev::BinaryBegin {
                op: crate::ev::BinOp::IfNull,
                span: tspan(1),
            },
            read(X),
            Ev::OtherExpr { span: tspan(2) },
            Ev::BinaryEnd {
                op: crate::ev::BinOp::IfNull,
                end: 3,
            },
        ],
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

/// The `??` skip-path promotion feeds the write's types-of-interest pass:
/// `int? x = g(); x = x ?? 0;` leaves `x` promoted to `int` (the written
/// type is the LHS's non-null form). The RHS itself evaluates on the
/// LHS-null path — unpromoted (official `ifNullExpression_rightBegin`,
/// flow_analysis.dart:7086-7104).
#[test]
fn if_null_write_repromotes_via_types_of_interest() {
    use crate::ev::Ev;
    use crate::ty::{Base, Ty};
    const INT: u32 = 102;
    const X: u32 = 7;
    let events: Vec<Ev> = [
        declared(X, named_ty(INT, true)),
        vec![
            read(X),
            Ev::BinaryBegin {
                op: crate::ev::BinOp::IfNull,
                span: tspan(1),
            },
            read(X),
            Ev::OtherExpr { span: tspan(2) },
            Ev::BinaryEnd {
                op: crate::ev::BinOp::IfNull,
                end: 3,
            },
            Ev::Assign { plain: true },
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

/// The true-branch promotion-step gates (official `tryPromoteToType` +
/// `isValidPromotionStep`, flow_analysis.dart:4137-4145, :7229-7246): a
/// redundant `x is int` on `int x` must NOT promote (the old factoring
/// pass fed `factor(int, int) = Never` straight into the chain, and an
/// `x is int?` on `int x` promoted to `int?`).
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
