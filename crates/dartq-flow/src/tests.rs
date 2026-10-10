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
            _ => None,
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
            _ => None,
        })
        .collect()
}

/// reachability basics: code after `return` is dead
/// (official reachability/data/early_exit.dart).
#[test]
fn dead_after_return() {
    let source = "void f() { return; 1; }";
    assert_eq!(dead_snippets(source), ["1"]);
}

#[test]
fn dead_after_return_in_if_branches() {
    // Both branches return: the join leaves nothing reachable.
    let source = "void f(bool b) { if (b) { return; } else { return; } 1; }";
    assert_eq!(dead_snippets(source), ["1"]);
}

#[test]
fn live_after_if_without_else_return() {
    let source = "void f(bool b) { if (b) { return; } 1; }";
    assert!(dead_snippets(source).is_empty());
}

/// official reachability/data/do.dart `do_true`/`do_false`: a `do` body
/// runs once; with a `true` condition only `break` reaches the exit.
#[test]
fn do_true_body_return_kills_exit() {
    let source = "void f() { do { return; } while (true); 1; }";
    assert_eq!(dead_snippets(source), ["1"]);
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

/// official definite_assignment/data/while.dart `condition_notTrue`: a
/// conditional `while` may run zero times.
#[test]
fn while_zero_iterations_leaves_unassigned() {
    let source = "void f(bool b) { int x; while (b) { x = 1; } x; }";
    assert_eq!(unassigned_snippets(source), ["x"]);
}

/// official definite_assignment/data/while.dart `true_break_afterAssignment`:
/// the only exits from `while (true)` are breaks; a break after the
/// assignment makes the read assigned.
#[test]
fn while_true_break_after_assignment() {
    let source = "void f(bool b) { int x; while (true) { x = 1; if (b) break; } x; }";
    assert!(unassigned_snippets(source).is_empty());
}

/// official definite_assignment/data/while.dart `true_break_beforeAssignment`.
#[test]
fn while_true_break_before_assignment() {
    let source = "void f(bool b) { int x; while (true) { if (b) break; x = 1; } x; }";
    assert_eq!(unassigned_snippets(source), ["x"]);
}

/// official definite_assignment/data/while.dart `true_continue`: continues
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

/// official definite_assignment/data/for.dart: a C-style `for` may run
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
    assert_eq!(dead_snippets(source), ["1"]);
}

/// official reachability/data/switch.dart `case_never_completes`: dead
/// code inside a case body whose both branches return.
#[test]
fn switch_case_inner_dead_code() {
    let source = "void f(bool b, int i) { switch (i) { case 1: 1; if (b) { return; } else { return; } 2; } 3; }";
    assert_eq!(dead_snippets(source), ["2"]);
}

/// official definite_assignment/data/closure.dart: writes inside closures
/// do not make outer reads assigned.
#[test]
fn closure_write_does_not_assign_outer_local() {
    let source = "void f() { int x; g() { x = 1; }; g(); x; }";
    assert_eq!(unassigned_snippets(source), ["x"]);
}

/// official definite_assignment/data/if.dart: assignment on every path
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

/// official definite_assignment/data/try.dart shape: a catch that returns
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

/// official definite_assignment/data/late.dart shape: reading a `late`
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

/// official type_promotion/data/null_check.dart shape: an `!= null` test
/// flows promotion into the then-branch; with the name-level seam this
/// must not crash and must not produce false dead code.
#[test]
fn null_check_promotion_no_facts() {
    let source = "void f(int? x) { if (x != null) { x; } }";
    assert!(facts_of(source).is_empty());
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
    assert_eq!(dead_snippets(source), ["1"]);
}

/// official reachability/data/while.dart shape: `while (true)` with no
/// break makes following code dead.
#[test]
fn while_true_no_break_kills_exit() {
    let source = "void f() { while (true) { 1; } 2; }";
    assert_eq!(dead_snippets(source), ["2"]);
}

/// Pattern variable declaration statement: the declared variables are
/// assigned on the statement's own path.
#[test]
fn pattern_declaration_assigns() {
    let source = "void f(Object o) { var (a, b) = (1, 2); a; b; }";
    assert!(unassigned_snippets(source).is_empty());
}

/// Closures inside loops: the loop-head conservative join must not crash
/// with captured writes, and facts stay absent for sound code.
#[test]
fn closure_in_loop_no_facts() {
    let source = "void f() { for (var i = 0; i < 3; i++) { g(() { i; }); } }";
    assert!(facts_of(source).is_empty());
}

/// A `try/finally` whose finally returns: the code after is dead
/// (official reachability/data/try_again shapes).
#[test]
fn finally_return_kills_exit() {
    let source = "void f() { try { 1; } finally { return; } 2; }";
    assert_eq!(dead_snippets(source), ["2"]);
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
#[ignore = "blocked: parser mis-parses collection-element for-heads (dartq-parser lane) — no ForInParts extent is produced to pin"]
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
#[ignore = "blocked: parser mis-parses collection-element for-heads (dartq-parser lane); stray VarDeclBegin clears pending_inits"]
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
    assert!(facts_of(source).is_empty());
}

/// An arrow body's value is consumed by the function and the function
/// expression leaves its own value: `InitializerEnd` finds a right-hand
/// side even when the body itself leaves nothing (a `throw` body). Before
/// the fix the `ExprBody` no-op shadow leaked the body's value — with a
/// `throw` body nothing was left and the variable stayed unwritten.
#[test]
fn arrow_body_nets_one_value() {
    let source = "void f() { var g = () => throw 1; g; }";
    assert!(unassigned_snippets(source).is_empty());
}

/// `suspension` demotes only inside a local function (official
/// `_enclosingFunctionExpressionInfoStack` non-empty): an await directly
/// in an async method body demotes nothing, so a promotion from the
/// condition survives the await without crashing.
#[test]
fn await_in_plain_async_method_demotes_nothing() {
    let source = "void f(int? x, Future g()) async { if (x != null) { await g(); x; } }";
    assert!(facts_of(source).is_empty());
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
        Ev::DoEnd,
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
    interp.run(events);
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
    interp.run(events);
    assert!(interp.stack.is_empty());
    assert_eq!(interp.exprs.len(), 1);
}

/// A closure's effect collect inside an enclosing loop replays the inner
/// loop's end event: that collect-mode `loop_exit` must not pop the
/// enclosing replay's REAL loop frame (the rollback truncates only the
/// tail, so a mid-stack removal loses the outer loop's exit model — its
/// cond-false path — for good, and code after the loop is misreported
/// dead). Shape: `while (a) { () { while (b) {} }(); return; } 1;` — the
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
        Ev::ExecEnd,
        Ev::ReturnBegin { span: span(3) },
        Ev::ReturnEnd { has_expression: false },
        Ev::WhileEnd,
        Ev::ExprStmt { span: span(4) },
    ];
    let mut interp = Interp::new(&NameTypeOps);
    interp.run(events);
    assert!(interp.stack.is_empty());
    assert!(interp.facts.is_empty());
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
    interp.run(events);
    assert!(interp.stack.is_empty());
    assert!(interp.facts.is_empty());
    assert!(interp.reachable());
}
