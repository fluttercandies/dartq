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
fn interp_of(events: Vec<crate::ev::Ev>) -> crate::interp::Interp<'static> {
    let mut interp = crate::interp::Interp::new(&crate::ty::NameTypeOps);
    interp.run(events);
    interp
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

/// official `getLabelTarget` (flow_analysis_visitor.dart:487): a labeled
/// break resolves to the labeled statement's loop, skipping the
/// intervening inner loop. Before label ids were carried on the events
/// this degraded to nearest-jump-target resolution: the break then exited
/// the INNER loop, `x = 1;` stayed live and the exit read came out
/// assigned.
#[test]
fn labeled_break_targets_labeled_loop_not_nearest() {
    let source =
        "void f(bool b) { int x; a: while (true) { while (true) { break a; } x = 1; } x; }";
    assert_eq!(dead_snippets(source), ["x"]);
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
/// `break b` must match through the recorded `LabelDecl`).
#[test]
fn labeled_break_matches_any_label_of_the_statement() {
    let source =
        "void f(bool b) { int x; a: b: while (true) { while (true) { break b; } x = 1; } x; }";
    assert_eq!(dead_snippets(source), ["x"]);
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

/// A cast pattern's primary is a pattern, not an expression: the parser
/// reuses the as-operator events for `p as T` (patterns.rs:176-187), so
/// `AsEnd` must not pop an operand off the empty expr stack (the "as
/// operand" panic), and the cast refines the pattern's declared variable
/// on the matched path (shared_type_analyzer.dart:145-152).
#[test]
fn cast_pattern_does_not_pop_the_expr_stack() {
    // Wildcard primary: nothing to pop, no crash, sound facts.
    let source = "void f(Object? o) { switch (o) { case _ as Object?: 1; } }";
    assert!(facts_of(source).is_empty());
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
    assert!(facts_of(source).is_empty());
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
        Ev::ExecEnd { is_expr: false },
        Ev::ReturnBegin { span: span(3) },
        Ev::ReturnEnd {
            has_expression: false,
        },
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

// —— try/catch/finally combinations ——————————————————————————————————————
// Event layouts mirror the recorder (record.rs): the try body runs before
// `CatchClauseBegin`, each clause ends at `CatchClauseEnd` (the
// `handle_catch_block` join), `FinallyBlock` separates the catch context
// from the finally body, and `TryEnd { has_finally }` closes the frame.

/// try + catch + finally, every part completing normally: `FinallyBlock`
/// joins the try-catch exit with the conservative exception-entry state
/// (official `tryFinallyStatement_finallyBegin`, flow_analysis.dart:8269)
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
    interp.run(events);
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
    interp.run(events);
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
    interp.run(events);
    assert!(interp.stack.is_empty());
    assert_eq!(interp.facts.len(), 1);
    assert!(!interp.reachable());
}

/// Two catch clauses: the first returns, the second completes. Later
/// catches resume from `beforeCatch`, not from the previous catch's exit
/// (official `tryCatchStatement_catchBegin`, flow_analysis.dart:8182), so
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
    interp.run(events);
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
    interp.run(events);
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
    interp.run(events);
    assert!(interp.stack.is_empty());
    assert!(interp.facts.is_empty());
    assert!(interp.reachable());
}

/// try + catch + finally where the CATCH returns but the try body
/// completes: the body's normal exit is seeded into the after-accumulator
/// (official `tryCatchStatement_bodyEnd`, flow_analysis.dart:8163) and
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
    interp.run(events);
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
/// flow_analysis.dart:1680).
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
    interp.run(events);
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
        },
        Ev::CaseExprBegin,
        Ev::OtherExpr { span: span(3) },
        Ev::CaseExprEnd { has_guard: false },
        Ev::ReturnBegin { span: span(4) },
        Ev::ReturnEnd {
            has_expression: false,
        },
        Ev::CaseEnd { statement_count: 1 },
        // `default:` — no case-expression events; the group opens at once.
        Ev::CaseBegin {
            label_count: 0,
            expression_count: 0,
        },
        Ev::ReturnBegin { span: span(5) },
        Ev::ReturnEnd {
            has_expression: false,
        },
        Ev::CaseEnd { statement_count: 1 },
        Ev::SwitchBlockEnd,
        Ev::ExprStmt { span: span(6) },
        Ev::OtherExpr { span: span(7) },
    ];
    let mut interp = Interp::new(&NameTypeOps);
    interp.run(events);
    assert!(interp.stack.is_empty());
    assert_eq!(interp.facts.len(), 1);
    assert!(!interp.reachable());
}

/// A switch expression used as a variable initializer must leave exactly
/// one value for the enclosing `InitializerEnd` consumer: `var y =
/// switch (x) { 1 => 2 }; y;` must not report `y` as definitely
/// unassigned.
///
/// Suspected defect (do not weaken): `SwitchExprCaseEnd` (interp_stmt.rs)
/// pops each arm's value and `SwitchExprBlockEnd` (interp_stmt.rs) never
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
    interp.run(events);
    assert!(interp.stack.is_empty());
    assert!(interp.exprs.is_empty());
    assert!(interp.facts.is_empty());
    assert!(interp.reachable());
}

/// official definite_assignment/data/assert.dart `assertStatement`: the
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
        Ev::BinaryBegin { op: BinOp::Other },
        Ev::OtherExpr { span: span(5) },
        Ev::BinaryEnd { op: BinOp::Other },
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
    interp.run(events);
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
    interp.run(events);
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
    interp.run(events);
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
        Ev::ExecEnd { is_expr: false },
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
    interp.run(events);
    assert!(interp.stack.is_empty());
    assert!(interp.facts.is_empty());
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
    assert!(facts_of(source).is_empty());
}

// —— for-in statements ————————————————————————————————————————————————————
// Event layout mirrors the recorder: the loop variable declaration and the
// iterable expression precede `ForInParts` (the parser fires
// `handle_for_in_loop_parts` after the iterable), the body sits between
// `ForInBodyBegin`/`ForInBodyEnd`, and `ForInEnd` closes the loop.

/// Statement for-in, real replay: the head pops the iterable, the body's
/// writes are conservatively joined at the loop head, and the exit joins
/// the zero-iteration state (official `forEach_bodyBegin`/
/// `forEach_end`, flow_analysis.dart:6941). Invariant-level on facts; the
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
    interp.run(events);
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
    interp.run(events);
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
    interp.run(events);
    assert!(interp.stack.is_empty());
    assert_eq!(interp.exprs.len(), 1);
    assert!(interp.facts.is_empty());
}

/// official definite_assignment/data/for_each.dart `forEach`: the iterable
/// is evaluated (and its writes hold) even when the loop runs zero times,
/// so only the body's write stays not-definite.
#[test]
fn for_each_zero_iteration_leaves_unassigned() {
    let source =
        "void f() { late Object v1, v2; for (var _ in (v1 = [0, 1, 2])) { v2 = 0; } v1; v2; }";
    assert_eq!(unassigned_snippets(source), ["v2"]);
}

/// official definite_assignment/data/for_each.dart `forEach_continue`:
/// continue paths re-enter the body, so neither the pre-continue write nor
/// the post-continue write is definite at the exit.
#[test]
fn for_each_continue_paths_unassigned() {
    let source = "void f(bool c) { late int v1, v2; for (var _ in [0, 1, 2]) { v1 = 0; if (c) continue; v2 = 0; } v1; v2; }";
    assert_eq!(unassigned_snippets(source), ["v1", "v2"]);
}

/// Reading the for-in loop variable inside the body must produce no fact:
/// the analyzer declares the loop variable initialized right before
/// `forEach_bodyBegin` (analyzer for_resolver.dart,
/// `declare(initialized: true)`; official corpus for_each.dart
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
/// interpreter's dead-code check (the `AssertBegin` span anchors).
#[test]
fn dead_assert_statement_reports_dead_code() {
    let source = "void f() { return; assert(true); }";
    assert_eq!(dead_snippets(source), ["assert"]);
}

/// A dead plain block reports one row spanning the whole block extent,
/// and statements entered inside it report nothing further: their
/// unreachability is implied by the outermost row (official dead_code
/// folds each dead region into one diagnostic,
/// `dead_code_verifier.dart` `_firstDead`).
#[test]
fn dead_statement_block_reports_outermost_row() {
    let source = "void f() { return; { var x = 1; x; } }";
    assert_eq!(dead_snippets(source), ["{ var x = 1; x; }"]);
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
            Ev::ExecEnd { is_expr: false },
        ],
    ]
    .concat();
    let interp = interp_of(events);
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
    let interp = interp_of(events);
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
            Ev::ExecEnd { is_expr: false },
        ],
    ]
    .concat();
    let interp = interp_of(events);
    assert!(
        matches!(interp.facts.as_slice(), [FlowFact::DeadCode { .. }]),
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
            },
            read(X),
            Ev::OtherExpr { span: tspan(2) },
            Ev::BinaryEnd {
                op: crate::ev::BinOp::IfNull,
            },
        ],
    ]
    .concat();
    let interp = interp_of(events);
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
            },
            read(X),
            Ev::OtherExpr { span: tspan(2) },
            Ev::BinaryEnd {
                op: crate::ev::BinOp::IfNull,
            },
            Ev::Assign { plain: true },
            Ev::ExecEnd { is_expr: false },
        ],
    ]
    .concat();
    let interp = interp_of(events);
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
        let interp = interp_of(events);
        assert_eq!(
            interp.current.model(0).promoted,
            NIL,
            "tested_nullable={tested_nullable}: a redundant `is` is not a promotion step"
        );
    }
}
