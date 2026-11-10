//! #3798 — dead_code / unnecessary_type_check face pins against the
//! oracle (Dart SDK 3.13.7).

use crate::facts::FlowFact;
use crate::forward::SharedFlow;
use crate::record::Flow;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_scanner::{ScanConfig, TokenTable};

/// The dead-code rows of `source` as `(offset, length, covered text)`,
/// with `staged` `(decl-offset, lexeme, nullable)` inferred-local types
/// layered in the way `Flow::stage_inferred_local` delivers them in the
/// full engine.
fn dead_rows(source: &str, staged: &[(u32, &str, bool)]) -> Vec<(u32, u32, String)> {
    use crate::ty::{Base, Ty};
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
    for &(offset, lexeme, nullable) in staged {
        let base = flow.0.borrow_mut().intern_name(lexeme);
        flow.0.borrow_mut().stage_inferred_local(
            offset,
            Ty {
                base: Base::Name(base),
                nullable,
            },
            Vec::new(),
        );
    }
    let mut parser = dartq_parser::Parser::new(source, tokens, features, &mut flow);
    parser.parse_unit();
    drop(parser);
    flow.0
        .borrow_mut()
        .take_facts()
        .into_iter()
        .filter_map(|fact| match fact {
            FlowFact::DeadCode { span } => {
                let start = span.range.offset as usize;
                let end = start + span.range.length as usize;
                Some((
                    span.range.offset,
                    span.range.length,
                    source[start..end].to_string(),
                ))
            }
            _ => None,
        })
        .collect()
}

/// A `T?` staged at the declaration's identifier offset — the way the
/// semantic layer answers `int? i` for the flow replay.
fn int_q(source: &str, name: &str) -> (u32, &'static str, bool) {
    (source.find(name).expect("decl name") as u32, "int", true)
}

/// A definitely-unassigned nullable local's `?.` row is NOT the flow
/// layer's: `_verifyUnassignedVariable` is its own reporting path
/// (`operator.offset .. node.end`, with the official parent climb), and
/// the semantic `dead_null_chains` latch owns it end to end. The flow
/// layer models the receiver only as its `unassigned` read fold, so it
/// must stay SILENT for `(i)?.sign` — a row here would duplicate the
/// semantic chain's span (`dead_code @?` twice, the #3172 regression).
#[test]
fn paren_receiver_null_aware_property_rows() {
    let source = "void f() {\n  int? i;\n  (i)?.sign;\n}\n";
    let rows = dead_rows(source, &[int_q(source, "i;")]);
    assert!(rows.is_empty(), "{rows:?}");
}

/// The bare twin of the face above (`i?.sign`): the same ownership split
/// keeps the flow layer silent.
#[test]
fn bare_receiver_null_aware_property_rows() {
    let source = "void f() {\n  int? i;\n  i?.sign;\n}\n";
    let rows = dead_rows(source, &[int_q(source, "i;")]);
    assert!(rows.is_empty(), "{rows:?}");
}

/// The `(`-anchored dead interval of an invocation whose callee itself
/// exited at `(` (`f(0)` over `Never f`): the official's first dead node
/// is the argument list, not the first argument
/// (`DeadCodeTest.test_invokeNever_functionExpressionInvocation_simpleIdentifier`,
/// oracle row `2:4|16` covering `(0);\n  print(1);`).
#[test]
fn never_param_call_anchors_at_open_paren() {
    let source = "void g(Never f) {\n  f(0);\n  print(1);\n}\n";
    let open = source.find("(0)").expect("open") as u32;
    let rows = dead_rows(source, &[]);
    assert_eq!(
        rows,
        vec![(open, 16, "(0);\n  print(1);".to_string())],
        "{rows:?}"
    );
}

/// The parenthesized callee twin (`(f)(0)` over `Never f`): the `)` is
/// the event before `(`, and the same `(`-anchored row stands
/// (`test_invokeNever_functionExpressionInvocation_parenthesizedExpression`,
/// oracle `2:6|16`).
#[test]
fn paren_callee_call_anchors_at_open_paren() {
    let source = "void h(Never f) {\n  (f)(0);\n  print(2);\n}\n";
    let open = source.find("(0)").expect("open") as u32;
    let rows = dead_rows(source, &[]);
    assert_eq!(
        rows,
        vec![(open, 16, "(0);\n  print(2);".to_string())],
        "{rows:?}"
    );
}

/// `??=` with a `null` literal RHS on a provably non-null LHS: the
/// operand is the official's first dead node (`l ??= null` rows
/// `null;` plus the rest of the dead tail — oracle `4:9|23` over
/// `null;\n  check(1, 2, 3);`). `Ev::Null` carries no span, so the
/// step-level operand latch stays empty and the anchor derives from
/// the assignment end (`end - 4`) — never the `1:1` fallback.
#[test]
fn if_null_assign_null_literal_rhs_anchors_at_operand() {
    let source =
        "void check(a, b, c) {}\nvoid f() {\n  final l = 1;\n  l ??= null;\n  check(1, 2, 3);\n}\n";
    let null_off = source.find("null").expect("null") as u32;
    let rows = dead_rows(source, &[]);
    assert_eq!(
        rows,
        vec![(null_off, 23, "null;\n  check(1, 2, 3);".to_string())],
        "{rows:?}"
    );
}

// `await (throw …)` dead-anchor divergence: oracle anchors the dead
// interval at the statement AFTER the await (`return id(42);`), dartq
// anchors at `await`. Root cause is `stmt_began_dead` being refreshed
// mid-statement by `Ev::AwaitEnd`/`Ev::YieldEnd` (statement-layer arms
// in `interp_stmt.rs`, which is outside this ticket's scope), so the
// await statement's own `ExprStmt` wrapper reads `began_dead == true`
// and self-anchors. Fix site: `interp_stmt.rs`'s `stmt_step` wrapper
// set must cover the suspension-end events (or the `AwaitEnd`/`YieldEnd`
// arms must move to the expression layer). Reported out of scope.

/// `nn ??= 2` on a provably non-null LHS (VALID write — `int` RHS into
/// `int` LHS): the official dead interval still runs from the RHS
/// operand to the covering close — the entry-dead null path makes `2`
/// the first dead node and `flowEnd(Block)` reports `2.offset ..
/// block.lastStatement.end` (oracle `5:10|9` = `2;\n  g();`). The
/// dead_null_aware row on `2` is a separate face.
#[test]
fn if_null_assign_int_rhs_dead_runs_to_close() {
    let source = "void g() {}\nvoid f() {\n  int nn = 0;\n  nn ??= 2;\n  g();\n}\n";
    let rhs = source.find("2;").expect("rhs") as u32;
    let rows = dead_rows(source, &[]);
    assert_eq!(rows, vec![(rhs, 9, "2;\n  g();".to_string())], "{rows:?}");
}

/// The string-literal sibling of the `??= null` face: `s ??= 'dead'`
/// on a non-null LHS anchors the dead interval at the operand —
/// `'dead'` through the covering close (oracle `5:9|14` =
/// `'dead';\n  g();`). `Ev::EndLiteralString` carries no span, so the
/// recorder emits a no-op `Ev::ParenBegin` carrying the simple
/// literal's token for the operand latch.
#[test]
fn if_null_assign_string_literal_rhs_anchors_at_operand() {
    let source = "void g() {}\nvoid f() {\n  String s = '';\n  s ??= 'dead';\n  g();\n}\n";
    let lit = source.find("'dead'").expect("lit") as u32;
    let rows = dead_rows(source, &[]);
    assert_eq!(
        rows,
        vec![(lit, 14, "'dead';\n  g();".to_string())],
        "{rows:?}"
    );
}

/// The `?.`-member dead interval over a `null` literal receiver:
/// `null?.m(1)` anchors at the member name (`m(1)`), NOT the operator —
/// the `?.`-anchor is exclusive to the definitely-unassigned nullable
/// local face (`dead_code_verifier._verifyUnassignedVariable`'s
/// `operator.offset` row; oracle `2:9|4` = `m(1)`).
#[test]
fn null_literal_receiver_null_aware_member_rows() {
    let source = "void f() {\n  null?.m(1);\n}\n";
    let member = source.find("m(1)").expect("m") as u32;
    let rows = dead_rows(source, &[]);
    assert_eq!(rows, vec![(member, 4, "m(1)".to_string())], "{rows:?}");
}

/// An ASSIGNED `Null`-typed local takes the same member-name anchor —
/// `x?.m(1)` on `Null x = null` rows `m(1)` (oracle `9:6|4`), the
/// unassigned-only `?.` anchor does not apply.
#[test]
fn null_typed_receiver_null_aware_member_rows() {
    let source = "void g() {\n  Null x = null;\n  x?.m(1);\n}\n";
    let member = source.find("m(1)").expect("m") as u32;
    let rows = dead_rows(source, &[]);
    assert_eq!(rows, vec![(member, 4, "m(1)".to_string())], "{rows:?}");
}

/// The method-invocation twin of the unassigned face: `i?.abs()` on an
/// unassigned `int? i` reports `?.abs()` (oracle `3:4|7`) — the semantic
/// chain's row. The flow layer stays silent, the same split the property
/// face above carries.
#[test]
fn unassigned_receiver_null_aware_call_rows() {
    let source = "void f() {\n  int? i;\n  i?.abs();\n}\n";
    let rows = dead_rows(source, &[int_q(source, "i;")]);
    assert!(rows.is_empty(), "{rows:?}");
}

/// The `?..` cascade head on a `null` literal target: the section is the
/// first dead node and its `beginToken` is the `?..` operator, so the row
/// opens at `?` and runs through the statement's `;` — `null?..m(1);`
/// reports `?..m(1);` (oracle `2:7|8`). The recorder delivers the operator
/// token as the phantom `Ev::ParenBegin` before `Ev::CascadeBegin`.
#[test]
fn null_literal_receiver_null_aware_cascade_rows() {
    let source = "void f() {\n  null?..m(1);\n}\n";
    let q = source.find("?..").expect("?..") as u32;
    let rows = dead_rows(source, &[]);
    assert_eq!(rows, vec![(q, 8, "?..m(1);".to_string())], "{rows:?}");
}

/// An ASSIGNED `Null` local's `?..` head anchors identically — the
/// `?..` operator is the dead interval's open (oracle `3:4|8` —
/// `x?..m(1);` rows `?..m(1);`), no `_verifyUnassignedVariable` needed.
#[test]
fn null_typed_receiver_null_aware_cascade_rows() {
    let source = "void j() {\n  Null x = null;\n  x?..m(1);\n}\n";
    let q = source.find("?..").expect("?..") as u32;
    let rows = dead_rows(source, &[]);
    assert_eq!(rows, vec![(q, 8, "?..m(1);".to_string())], "{rows:?}");
}

/// The `?..` cascade head over a definitively-unassigned nullable target:
/// `_verifyUnassignedVariable`'s cascade arm (`verifyCascadeExpression`)
/// gives the semantic chain the `?`-through-cascade-end row — `i?..abs()`
/// rows `?..abs()` (oracle `3:4|8`) — so the flow layer stays silent. The
/// reachability receivers (the `null`-literal and `Null`-typed pins
/// below) still anchor here.
#[test]
fn unassigned_nullable_receiver_null_aware_cascade_rows() {
    let source = "void k() {\n  int? i;\n  i?..abs();\n}\n";
    let rows = dead_rows(source, &[int_q(source, "i;")]);
    assert!(rows.is_empty(), "{rows:?}");
}

/// A multi-section `?..` cascade on a dead target reports ONE interval
/// from the head `?..` through the statement's `;` — `x?..m(1)?..n(2);`
/// rows `?..m(1)?..n(2);` (oracle `4:4|15`); the out-of-order `?..` is a
/// separate syntactic error, and the head short spans all sections
/// (upstream opens one `_nullAwareAccess_rightBegin` per cascade).
#[test]
fn multi_section_null_aware_cascade_rows() {
    let source = "void p() {\n  Null x = null;\n  x?..m(1)?..n(2);\n}\n";
    let q = source.find("?..").expect("?..") as u32;
    let rows = dead_rows(source, &[]);
    assert_eq!(
        rows,
        vec![(q, 15, "?..m(1)?..n(2);".to_string())],
        "{rows:?}"
    );
}
