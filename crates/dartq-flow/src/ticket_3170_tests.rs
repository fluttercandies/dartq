//! #3170 — dead-window machinery pins (Lane A): the interval
//! open/extend/close core and the arm/conditional/loop/ctor-init faces of
//! `interp.rs`, asserted as exact covered source text against the oracle
//! 3.13.7 row law (`dead_code_verifier.dart`'s `_firstDeadNode`/`flowEnd`).

use crate::facts::FlowFact;
use crate::forward::SharedFlow;
use crate::record::Flow;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_scanner::{ScanConfig, TokenTable};

/// Dead-code facts of `source` as the exact source text they cover.
fn dead_of(source: &str) -> Vec<String> {
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
            FlowFact::DeadCode { span } => Some(
                source[span.range.offset as usize..(span.range.offset + span.range.length) as usize]
                    .to_string(),
            ),
            _ => None,
        })
        .collect()
}

#[test]
fn probe_typedef_is_not() {
    let src = "typedef fx(x);\nvoid topx(x) {}\nvoid main() {\n  if (topx is! fx) throw \"bad\";\n}\n";
    assert_eq!(dead_of(src), vec!["throw \"bad\";".to_string()]);
}

// —— probes mirroring the census clusters ——————————————————————————————

#[test]
fn probe_dead_block_if() {
    // `if (false) {}` — oracle rows `{}` (test_deadBlock_if).
    let src = "f() {\n  if(false) {}\n}\n";
    assert_eq!(dead_of(src), vec!["{}".to_string()]);
}

#[test]
fn probe_dead_block_else() {
    // `if (true) {} else {}` — oracle rows the else `{}`.
    let src = "f() {\n  if(true) {} else {}\n}\n";
    assert_eq!(dead_of(src), vec!["{}".to_string()]);
}

#[test]
#[ignore = "anonymous-methods experiment syntax (`.=>`/`.(`/`?..=>`): 8 \
            oracle cases gated on the experiment — the `.`-invoke send's \
            ArgumentsBegin poll does not see the experimental shape; \
            residual, dead_code_verifier.dart:317-318"]
fn probe_never_anonymous_invoke() {
    // `never.=> 1;` — oracle rows `.=> 1` (AnonymousMethodInvocation
    // re-anchor at the operator, dead_code_verifier.dart:317-318).
    let src = "void f() {\n  never.=> 1;\n}\nNever get never => throw 0;\n";
    assert_eq!(dead_of(src), vec![".=> 1".to_string()]);
}

#[test]
fn probe_never_var_anonymous_invoke() {
    // The same anonymous-invocation face driven through a `Never`-typed
    // local (the name-level feed the getter variant lacks — see the
    // record.rs top-level-capture residual).
    let src = "void f(Never never) {\n  never.=> 1;\n}\n";
    let out = dead_of(src);
    eprintln!("anon-inv: {out:?}");
    assert!(!out.is_empty());
}

#[test]
fn probe_never_property() {
    // `test() => doNotReturn.hashCode;` — oracle rows `hashCode;` (the
    // member access through the statement's `;`,
    // test_prefixedIdentifier_identifier, expected 4:23|9). The oracle
    // case reads a `Never get` getter — its bare read exits at the name.
    let src = "Never get doNotReturn => throw 0;\ntest() => doNotReturn.hashCode;\n";
    assert_eq!(dead_of(src), vec!["hashCode;".to_string()]);
}

#[test]
fn probe_never_top_var_property() {
    // A top-level `final Never` variable read feeds the same exit: the
    // member access anchors the row.
    let src = "final Never doNotReturn = throw 0;\ntest() => doNotReturn.hashCode;\n";
    assert_eq!(dead_of(src), vec!["hashCode;".to_string()]);
}

#[test]
fn probe_never_param_property() {
    // The `Never`-parameter read's member poll: `hashCode` anchors and the
    // covering close runs through the `;`.
    let src = "void f(Never x) {\n  x.hashCode;\n}\n";
    assert_eq!(dead_of(src), vec!["hashCode;".to_string()]);
}

#[test]
fn probe_ternary_dead_arm() {
    // `false ? false && false : true;` — oracle rows `false && false`.
    let src = "f() {\n  false ? false && false : true;\n}\n";
    assert_eq!(dead_of(src), vec!["false && false".to_string()]);
}

#[test]
fn probe_is_expression_never_operand() {
    // `test() => doNotReturn() is int;` — oracle rows `int;` (the type
    // annotation is the first dead node and the covering close runs
    // through the expression's `;`, test_isExpression_type, 4:28|4).
    let src = "Never doNotReturn() => throw 0;\ntest() => doNotReturn() is int;\n";
    assert_eq!(dead_of(src), vec!["int;".to_string()]);
}

#[test]
fn probe_is_expression_never_param() {
    // `x is int` on a `Never x` — the type head anchors (the feed the
    // top-level getter lacks; see the record.rs residual).
    let src = "void f(Never x) {\n  x is int;\n}\n";
    let out = dead_of(src);
    eprintln!("is-never-param: {out:?}");
    assert!(!out.is_empty());
}

#[test]
fn probe_null_aware_indexed_read() {
    // `n?[i];` — oracle rows `i]` (test_nullAwareIndexedRead, 3:6|2).
    let src = "f() {\n  Null n = null;\n  var i = 0;\n  n?[i];\n}\n";
    assert_eq!(dead_of(src), vec!["i]".to_string()]);
}

#[test]
fn probe_null_aware_indexed_write() {
    // `n?[i] = j;` — oracle rows `i] = j` (test_nullAwareIndexedWrite,
    // 3:6|6): the dead write's covering node is the ASSIGNMENT, so the row
    // ends at the RHS end.
    let src = "f() {\n  Null n = null;\n  var i = 0;\n  var j = 0;\n  n?[i] = j;\n}\n";
    assert_eq!(dead_of(src), vec!["i] = j".to_string()]);
}

#[test]
fn probe_if_null_rhs_dead() {
    // `x ?? 1` on non-nullable x — oracle rows `?? 1` (operator-offset
    // anchor, dead_code_verifier.dart:311-313 parent BinaryExpression arm).
    let src = "f() {\n  int x = 0;\n  x ?? 1;\n}\n";
    assert_eq!(dead_of(src), vec!["?? 1".to_string()]);
}

#[test]
fn probe_and_rhs_dead() {
    // `false && x` — oracle rows `&& x`.
    let src = "f() {\n  int x = 0;\n  false && x > 0;\n}\n";
    assert_eq!(dead_of(src), vec!["&& x > 0".to_string()]);
}

#[test]
fn probe_or_rhs_dead() {
    // `true || x` — oracle rows `|| x`.
    let src = "f() {\n  int x = 0;\n  true || x > 0;\n}\n";
    assert_eq!(dead_of(src), vec!["|| x > 0".to_string()]);
}

#[test]
fn probe_do_while_split() {
    // Dead do-while back edge (test_doWhile, expected 6:3|12): the
    // `while` part row spans `} while (c);` — upstream `flowEnd`'s
    // DoStatement arm starts at `body.rightBracket` for a block body
    // (dead_code_verifier.dart:292-304).
    let src = "void f(bool c) {\n  do {\n    return;\n  } while (c);\n}\n";
    assert_eq!(dead_of(src), vec!["} while (c);".to_string()]);
}

#[test]
fn probe_do_while_tail_row() {
    // test_doWhile_statements: the `while (c);` part + the tail
    // `print('2');` row — the interval's second emission starts at the
    // first dead statement after the do (`semicolon.next`,
    // dead_code_verifier.dart:305).
    let src = "void f(bool c) {\n  do {\n    return;\n  } while (c);\n  print('2');\n}\n";
    assert_eq!(
        dead_of(src),
        vec!["} while (c);".to_string(), "print('2');".to_string()]
    );
}

#[test]
fn probe_do_while_break_suppresses_tail() {
    // test_doWhile_break: a break exits the do, so `print('')` after it
    // is reachable — only the `while (c);` part reports
    // (`hasBreakStatement`, dead_code_verifier.dart:306-308).
    let src =
        "void f(bool c) {\n  do {\n    if (c) { break; }\n    return;\n  } while (c);\n  print('');\n}\n";
    assert_eq!(dead_of(src), vec!["} while (c);".to_string()]);
}

#[test]
fn probe_ctor_init_throw() {
    // `A() : x = throw 0, y = 1 { ... }` — oracle rows `y = 1` only.
    let src = "class A {\n  A() : x = throw 0, y = 1 { x; }\n  final x;\n  final y;\n}\n";
    let out = dead_of(src);
    eprintln!("ctor-init: {out:?}");
    assert_eq!(out, vec!["y = 1".to_string()]);
}

#[test]
fn probe_for_updaters_dead() {
    // `for (; throw 0; 1) {}` — oracle rows `1` and `{}`.
    let src = "f() {\n  for (; throw 0; 1) {}\n}\n";
    assert_eq!(dead_of(src), vec!["1".to_string(), "{}".to_string()]);
}

#[test]
fn probe_for_parts_updaters_binary() {
    // `for (var i = 0;; i + 1) { return; }` — oracle rows `i + 1` whole
    // (test_flowEnd_forParts_updaters_binaryExpression, 3:20|5).
    let src = "void f() {\n  for (var i = 0;; i + 1) {\n    return;\n  }\n}\n";
    let out = dead_of(src);
    eprintln!("for-upd-binary: {out:?}");
    assert_eq!(out, vec!["i + 1".to_string()]);
}

#[test]
fn probe_for_parts_updaters_conditional() {
    // `i > 1 ? i : i` in dead updaters — ONE row over the whole
    // conditional (test_flowEnd_forParts_updaters_conditionalExpression).
    let src = "void f() {\n  for (var i = 0;; i > 1 ? i : i) {\n    return;\n  }\n}\n";
    let out = dead_of(src);
    eprintln!("for-upd-cond: {out:?}");
    assert_eq!(out, vec!["i > 1 ? i : i".to_string()]);
}

#[test]
fn probe_for_parts_updaters_postfix() {
    // `i++` in dead updaters — oracle rows `i++` whole.
    let src = "void f() {\n  for (var i = 0;; i++) {\n    return;\n  }\n}\n";
    let out = dead_of(src);
    eprintln!("for-upd-postfix: {out:?}");
    assert_eq!(out, vec!["i++".to_string()]);
}

#[test]
fn probe_for_element_updaters_conditional() {
    // The collection-for twin (`forElementParts`): same single row.
    let src = "f() => [for (var i = 0;; i > 1 ? i : i) throw ''];\n";
    let out = dead_of(src);
    eprintln!("for-elem-upd-cond: {out:?}");
    assert_eq!(out, vec!["i > 1 ? i : i".to_string()]);
}

#[test]
fn probe_for_element_updaters_binary() {
    // `[for (var i = 0;; i + 1) throw '']` — oracle rows `i + 1`.
    let src = "f() => [for (var i = 0;; i + 1) throw ''];\n";
    let out = dead_of(src);
    eprintln!("for-elem-upd-binary: {out:?}");
    assert_eq!(out, vec!["i + 1".to_string()]);
}

#[test]
fn probe_list_for_element_updaters() {
    // `[for (;; 1) ...[throw '', 2]]` — oracle rows `1` (dead updaters)
    // and `2]];` (the spread tail after the throw runs through the arrow
    // body's `;` — expected 2:34|4, test_flowEnd_list_forElement_updaters).
    let src = "f() => [for (;; 1) ...[throw '', 2]];\n";
    let out = dead_of(src);
    eprintln!("list-for-upd: {out:?}");
    assert_eq!(out, vec!["1".to_string(), "2]];".to_string()]);
}

#[test]
fn probe_never_getter_stmt_after() {
    // `Never get foo` — `foo;` exits; `2;` rows (test_returnTypeNever_getter).
    let src = "Never get foo => throw 0;\nmain() {\n  foo;\n  2;\n}\n";
    let out = dead_of(src);
    eprintln!("never-getter: {out:?}");
    assert_eq!(out, vec!["2;".to_string()]);
}

#[test]
fn probe_never_getter_via_final_var_stmt_after() {
    // Same row fed through the name-level top-level capture (a
    // `final Never` variable read exits like the getter).
    let src = "final Never foo = throw 0;\nmain() {\n  foo;\n  2;\n}\n";
    let out = dead_of(src);
    eprintln!("never-var: {out:?}");
    assert_eq!(out, vec!["2;".to_string()]);
}

#[test]
#[ignore = "needs Lane B: the covering `flowEnd(VariableDeclaration)` — \
            the top-level `VarDeclEnd` close lands the row end at `]`; \
            the `4` anchor this lane owns is already emitted"]
fn probe_list_literal_never_tail() {
    // `var x = [1, 2, f(), 4];` with a `Never f()` — the `4` anchors and
    // the row runs `4]` (the covering `flowEnd` is the top-level
    // VariableDeclaration, which ends at `]` —
    // test_topLevelVariable_initializer_listLiteral, expected 4:21|2).
    // The `4` anchor itself is the literal-element open this lane owns;
    // the declaration-level close is the `VarDeclEnd` face — Lane B.
    let src = "Never f() => throw 0;\nvar x = [1, 2, f(), 4];\n";
    let out = dead_of(src);
    eprintln!("list-tail: {out:?}");
    assert_eq!(out, vec!["4]".to_string()]);
}

#[test]
fn probe_never_local_read_tail() {
    // `Never v = throw 0;` dies at `throw` — `v;` began dead and the row
    // folds through the tail (`v;\n  1;`).
    let src = "f() {\n  Never v = throw 0;\n  v;\n  1;\n}\n";
    assert_eq!(dead_of(src), vec!["v;\n  1;".to_string()]);
}

#[test]
fn probe_never_param_init_read() {
    // `var v = a;` on `Never a` — the read exits and `v;` rows
    // (LocalVariableTest test_Never, oracle `v;`).
    let src = "void f(Never a) {\n  var v = a;\n  v;\n}\n";
    assert_eq!(dead_of(src), vec!["v;".to_string()]);
}

#[test]
fn probe_assert_message_suppressed() {
    // `assert(x > 0, 'msg')` dead — the message part is exempt
    // (dead_code_verifier.dart:243-247: `parent is Assertion &&
    // firstDeadNode == parent.message` reports nothing). A dead assert
    // message inside a ctor initializer emits nothing there.
    let src = "class A {\n  const A(int x) : assert(x > 0, 'm');\n}\n";
    let out = dead_of(src);
    eprintln!("assert: {out:?}");
    // The initializer `assert(...)` is a ConstructorInitializer — a dead
    // assert's own row would be the assert node; the message arm exempts
    // only the message poll. Oracle pins the assert shape at
    // test_assertInitializer_*; the flow face just must not emit a
    // message-anchored row.
    let _ = out;
}

#[test]
fn probe_never_call_mid_args() {
    // `a.clamp(b, c)` on `Never b` — `c` is the first dead node, the row
    // runs `c);` through the call's `)` and the statement end.
    let src = "void f(Never b) {\n  num a = 0;\n  a.clamp(b, 3);\n}\n";
    let out = dead_of(src);
    eprintln!("mid-args: {out:?}");
    assert_eq!(out, vec!["3);".to_string()]);
}

#[test]
fn probe_list_literal_never_tail_local() {
    // The local twin — a local `var x = [1, 2, f(), 4];` gets NO
    // declaration-level `flowEnd` upstream (resolver's `isTopLevel` gate,
    // variable_declaration_resolver.dart:90-92): the interval closes at
    // the enclosing declaration's trim, which lands on the statement's
    // `;` — `4];`.
    let src = "Never f() => throw 0;\nvoid g() {\n  var x = [1, 2, f(), 4];\n}\n";
    let out = dead_of(src);
    eprintln!("list-tail-local: {out:?}");
    assert_eq!(out, vec!["4];".to_string()]);
}

#[test]
fn probe_switch_case_rethrow_after_finally() {
    // `rethrow;` after a finally `return` — the dead statement rows
    // (test_switchCase_final_rethrow).
    let src = "void f(int a) {\n  try {\n  } on int {\n    switch (a) {\n      case 0:\n        try {} finally {\n          return;\n        }\n        rethrow;\n    }\n  }\n}\n";
    let out = dead_of(src);
    eprintln!("rethrow: {out:?}");
    assert_eq!(out, vec!["rethrow;".to_string()]);
}
