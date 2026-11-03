use super::{Check, NodeKind, PayloadField, RuleSpec};
use crate::rules::{const_ability, variables_declaration_keyword};
use dartq_parser::MemberKind;

/// Official: `pkg/linter/lib/src/rules/prefer_const_declarations.dart:52-64`
/// — a `final` declaration list whose every variable carries an
/// initializer that is a constant as written reports at the list
/// (:62). The visited faces (:31-34) are the static field declaration,
/// the top-level variable and the local statement.
///
/// The initializer gate (:55-60) folds to the const-ability face
/// (`const_ability::is_const_able`) with the as-written cut: the
/// official `hasConstantError` answers "constant" only for expressions
/// that already are one — so a creation without the written `const`
/// (`C()`, `new C()`, `C<int>(…)`) answers false, and the
/// `TypedLiteral` arm (:58) keeps bare collection literals silent (a
/// `[]` initializer is a runtime literal). Records and parenthesized
/// expressions are not `TypedLiteral`s and ride the const-ability
/// face. Approximations (documented — the evaluator cut): a
/// written-`const` wrapper answers true without validating its
/// contents (a constant error inside stays an over-report), and the
/// constant-reference faces resolve same-file only — tear-offs
/// (`C.new`, `C.m`, a bare function name) under-report. Message:
/// `diagnostic.g.dart` `preferConstDeclarations`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "prefer_const_declarations",
    &[NodeKind::VariablesDeclaration],
    check,
);

fn check(c: &mut Check<'_>) {
    let node = c.node();
    // `node.isConst`/`!node.isFinal` (:53-54) — the keyword must spell
    // exactly `final` (`const` and `var` never qualify).
    if variables_declaration_keyword(c, node).is_none_or(|kw| c.lexeme(kw) != "final") {
        return;
    }
    // The visited faces (:31-34). Instance fields stay silent (the
    // official registers only the static `FieldDeclaration`), and the
    // for-loop faces are no `VariableDeclarationStatement`s.
    let Some(parent) = c.ancestor(1) else {
        return;
    };
    match c.kind(parent) {
        NodeKind::Fields => {
            if c.field(parent, PayloadField::MemberKind) != Some(MemberKind::StaticField as u32) {
                return;
            }
        }
        NodeKind::TopLevelFields
        | NodeKind::Block
        | NodeKind::BlockBody
        | NodeKind::LabeledStatement
        | NodeKind::SwitchCase => {}
        _ => return,
    }
    // `node.variables.every(...)` (:55-61): every variable carries an
    // initializer that is a constant as written — one miss skips the
    // report (`final x, y = 1;`). The written-`const` wrapper
    // qualifies outright (:58-59 — the keyword arm); its content may
    // tile beside the wrapper row, so the initializer kids are read
    // wrapper-first.
    let mut any_variable = false;
    for var in c.collect_direct(node) {
        if c.kind(var) != NodeKind::InitializedIdentifier {
            continue;
        }
        let Some(init) = c
            .collect_direct(var)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::VariableInitializer)
        else {
            return; // a missing initializer skips the report (:56-57)
        };
        let kids = c.collect_direct(init);
        let wrapped = kids.iter().any(|&kid| {
            matches!(
                c.kind(kid),
                NodeKind::ConstExpression | NodeKind::ConstLiteral
            )
        });
        let verdict = if wrapped {
            Some(true)
        } else {
            // The tear-off/creation expression: the LAST send owns the
            // whole access (`C<int>.new` tiles the generic `C<int>`
            // send beside the `.new` tail send — the tail is the
            // constant face).
            kids.iter()
                .rev()
                .find(|&&kid| c.kind(kid) == NodeKind::Send)
                .or_else(|| kids.last())
                .copied()
                .map(|expr| const_as_written(c, expr))
        };
        if !verdict.unwrap_or(false) {
            return;
        }
        any_variable = true;
    }
    if !any_variable {
        return;
    }
    // `rule.reportAtNode(node)` (:62) — the list node: the keyword head
    // through the last variable's last token, the trailing `;` excluded
    // (the statement/field wrapper owns it; flat-tailed initializer
    // groups ride the row's own token range).
    if let Some(range) = super::variables_list_range(c, node) {
        c.report(range, &[]);
    }
}

/// Whether the initializer is a constant as written — the official
/// `initializer != null && (is! TypedLiteral || const keyword) &&
/// !hasConstantError` face (:56-60), minus the wrapper arm (its
/// caller reads the written `const` first).
fn const_as_written(c: &Check<'_>, expr: u32) -> bool {
    match c.kind(expr) {
        // A `TypedLiteral` without the `const` keyword (:58); a
        // creation is a constant only when `const` is written, so `new`
        // and the implicit-creation spellings never are either.
        NodeKind::ListLiteral
        | NodeKind::SetOrMapLiteral
        | NodeKind::NewExpression
        | NodeKind::ImplicitCreation => false,
        NodeKind::Send => !send_invocation(c, expr) && const_ability::is_const_able(c.unit, expr),
        _ => const_ability::is_const_able(c.unit, expr),
    }
}

/// Whether the send row carries an argument list — the invocation
/// faces are runtime shapes without the written `const` (a bare `C()`
/// with a const constructor is not a constant as written).
fn send_invocation(c: &Check<'_>, send: u32) -> bool {
    let Some(parent) = c.unit.ast.parent(send) else {
        return false;
    };
    let kids = c.collect_direct(parent);
    let Some(pos) = kids.iter().position(|&kid| kid == send) else {
        return false;
    };
    pos > 0 && c.kind(kids[pos - 1]) == NodeKind::Arguments
}

/// The #3308 oracle-corpus pins: every upstream case (the exact
/// materialized conformance sources — all single-file) with the oracle's
/// own-row spans.
#[cfg(test)]
mod tests {
    use crate::testsupport::{Fixture, one_rule};

    /// The whole upstream oracle corpus — `PreferConstDeclarationsTest` (47
    /// cases, `pkg/linter/test/rules/prefer_const_declarations_test.dart`)
    /// plus the four `messages.yaml` doc-bad and two doc-good cases: 53 total,
    /// `conformance/status.tsv` `lint prefer_const_declarations 53 36 17`.
    /// Each entry is the exact materialized case source and the oracle's
    /// `(line, column, length)` for every rule row.
    ///
    /// 53 single-file cases pinned (29 oracle rows), every mismatch collected before the single panic.
    #[test]
    fn prefer_const_declarations_oracle_corpus_pins() {
        // (case, source, oracle rows as (line, column, length)).
        let cases: &[(&str, &str, &[(u32, u32, u32)])] = &[
            (
                "test_constructorTearoff_inference#0",
                "\nvoid f() {\n  final C<int> Function() c = C.new;\n}\nclass C<T> {}",
                &[(3, 3, 33)],
            ),
            (
                "test_constructorTearoff_instantiatedWithType#0",
                "\nvoid f() {\n  final c = C<int>.new;\n}\nclass C<T> {}",
                &[(3, 3, 20)],
            ),
            (
                "test_constructorTearoff_instantiatedWithTypeVariable#0",
                "\nvoid f<T>() {\n  final c = C<T>.new;\n}\nclass C<T> {}",
                &[(3, 3, 18)],
            ),
            (
                "test_constructorTearoff_uninstantiated#0",
                "\nvoid f() {\n  final x = C.new;\n}\nclass C<T> {}",
                &[(3, 3, 15)],
            ),
            (
                "test_instanceField_final_listLiteral_const#0",
                "\nclass C {\n  final x = const [];\n}",
                &[],
            ),
            (
                "test_localVariable_const_listLiteral_const#0",
                "\nvoid f() {\n  const x = const [];\n}",
                &[],
            ),
            (
                "test_localVariable_final_constructorInvocation_const#0",
                "\nvoid f() {\n  final x = const C();\n}\nclass C {\n  const C();\n}",
                &[(3, 3, 19)],
            ),
            (
                "test_localVariable_final_constructorInvocation_new#0",
                "\nvoid f() {\n  final x = C();\n}\nclass C {\n  const C();\n}",
                &[],
            ),
            (
                "test_localVariable_final_doubleLiteral#0",
                "\nvoid f() {\n  final x = 1.3;\n}",
                &[(3, 3, 13)],
            ),
            (
                "test_localVariable_final_intLiteral#0",
                "\nvoid f() {\n  final x = 1;\n}",
                &[(3, 3, 11)],
            ),
            (
                "test_localVariable_final_intLiteral_multiple#0",
                "\nvoid f() {\n  final x, y = 1;\n}",
                &[],
            ),
            (
                "test_localVariable_final_listLiteral#0",
                "\nvoid f() {\n  final x = [];\n}",
                &[],
            ),
            (
                "test_localVariable_final_listLiteral_const#0",
                "\nvoid f() {\n  final x = const [];\n}",
                &[(3, 3, 18)],
            ),
            (
                "test_localVariable_final_mapOrSetLiteral_const#0",
                "\nvoid f() {\n  final x = const {};\n}",
                &[(3, 3, 18)],
            ),
            (
                "test_localVariable_final_methodInvocation#0",
                "\nvoid f() {\n  final x = 7.toString();\n}",
                &[],
            ),
            (
                "test_localVariable_final_nullLiteral#0",
                "\nvoid f() {\n  final x = null;\n}",
                &[(3, 3, 14)],
            ),
            (
                "test_localVariable_final_prefixedIdentifier#0",
                "\nvoid f() {\n  final x = 7.hashCode;\n}",
                &[],
            ),
            (
                "test_localVariable_final_staticProperty_const#0",
                "\nvoid f() {\n  final x = C.p;\n}\nclass C {\n  const C();\n  static const p = const [];\n}",
                &[(3, 3, 13)],
            ),
            (
                "test_localVariable_final_stringLiteral#0",
                "\nvoid f() {\n  final x = '';\n}",
                &[(3, 3, 12)],
            ),
            (
                "test_localVariable_listLiteral_final_typed#0",
                "\nvoid f() {\n  final List<int> x = [];\n}",
                &[],
            ),
            (
                "test_localVariable_listLiteral_final_typed_typeArg#0",
                "\nvoid f() {\n  final List<int> x = <int>[];\n}",
                &[],
            ),
            (
                "test_localVariable_listLiteral_final_untyped_typeArg#0",
                "\nvoid f() {\n  final x = <int>[];\n}",
                &[],
            ),
            (
                "test_localVariable_mapLiteral_final_typed#0",
                "\nvoid f() {\n  final Map<int,int> x = {};\n}",
                &[],
            ),
            (
                "test_localVariable_mapLiteral_final_typed_typeArgs#0",
                "\nvoid f() {\n  final Map<int,int> x = <int,int>{};\n}",
                &[],
            ),
            (
                "test_localVariable_mapLiteral_final_untyped_typeArgs#0",
                "\nvoid f() {\n  final x = <int,int>{};\n}",
                &[],
            ),
            (
                "test_localVariable_mapOrSetLiteral_final_untyped#0",
                "\nvoid f() {\n  final x = {};\n}",
                &[],
            ),
            (
                "test_localVariable_setLiteral_final_typed#0",
                "\nvoid f() {\n  final Set<int> x = {};\n}",
                &[],
            ),
            (
                "test_localVariable_setLiteral_final_typed_typeArg#0",
                "\nvoid f() {\n  final Set<int> ids2 = <int>{};\n}",
                &[],
            ),
            (
                "test_recordLiteral#0",
                "\nfinal tuple = const (\"first\", 2, true);",
                &[(2, 1, 38)],
            ),
            (
                "test_staticField_const_listLiteral_const#0",
                "\nclass C {\n  static const x = const [];\n}",
                &[],
            ),
            (
                "test_staticField_final_listLiteral#0",
                "\nclass C {\n  static final x = const [];\n}",
                &[(3, 10, 18)],
            ),
            (
                "test_staticField_final_listLiteral_const#0",
                "\nclass C {\n  static final x = const [];\n}",
                &[(3, 10, 18)],
            ),
            (
                "test_staticField_final_nullLiteral#0",
                "\nclass C {\n  static final x = null;\n}",
                &[(3, 10, 14)],
            ),
            (
                "test_staticFunctionTearoff_inference#0",
                "\nvoid f() {\n  final C<int> Function() c = C.m;\n}\nclass C<T> {\n  static C<X> m<X>() => C<X>();\n}",
                &[(3, 3, 31)],
            ),
            (
                "test_staticFunctionTearoff_instantiatedWithType#0",
                "\nvoid f() {\n  final c = C.m<int>;\n}\nclass C<T> {\n  static C<X> m<X>() => C<X>();\n}",
                &[(3, 3, 18)],
            ),
            (
                "test_staticFunctionTearoff_instantiatedWithTypeVariable#0",
                "\nvoid f<T>() {\n  final c = C.m<T>;\n}\nclass C<T> {\n  static C<X> m<X>() => C<X>();\n}",
                &[],
            ),
            (
                "test_staticFunctionTearoff_uninstantiated#0",
                "\nvoid f() {\n  final x = C.m;\n}\nclass C<T> {\n  static C<X> m<X>() => C<X>();\n}",
                &[(3, 3, 13)],
            ),
            (
                "test_test_recordLiteral_nonConst#0",
                "\nfinal tuple = (1, () {});",
                &[],
            ),
            (
                "test_test_recordLiteral_ok#0",
                "\nconst record = (number: 123, name: \"Main\", type: \"Street\");",
                &[],
            ),
            (
                "test_topLevelVariable_final_doubleLiteral#0",
                "\nfinal x = 1.3;",
                &[(2, 1, 13)],
            ),
            (
                "test_topLevelVariable_final_listLiteral#0",
                "\nfinal x = [];",
                &[],
            ),
            (
                "test_topLevelVariable_final_listLiteral_const#0",
                "\nfinal x = const [];",
                &[(2, 1, 18)],
            ),
            (
                "test_topLevelVariable_final_mapOrSetLiteral#0",
                "\nfinal o9 = {};",
                &[],
            ),
            (
                "test_topLevelVariable_final_mapOrSetLiteral_const#0",
                "\nfinal x = const {};",
                &[(2, 1, 18)],
            ),
            (
                "test_topLevelVariable_final_nullLiteral#0",
                "\nfinal a = null;",
                &[(2, 1, 14)],
            ),
            (
                "test_topLevelVariable_final_topLevelVariable_const#0",
                "\nconst x = const [];\nfinal y = x;",
                &[(3, 1, 11)],
            ),
            (
                "test_topLevelVariable_listLiteral_const#0",
                "\nconst x = const [];",
                &[],
            ),
            ("doc_bad_0", "final v = const <int>[];", &[(1, 1, 23)]),
            (
                "doc_bad_1",
                "class C {\n  static final f = const <int>[];\n}",
                &[(2, 10, 23)],
            ),
            (
                "doc_bad_2",
                "void f() {\n  final v = const <int>[];\n  print(v);\n}",
                &[(2, 3, 23)],
            ),
            (
                "doc_bad_3",
                "final o = const <int>[];\n\nclass A {\n  static final o = const <int>[];\n}",
                &[(1, 1, 23), (4, 10, 23)],
            ),
            (
                "doc_good_0",
                "class C {\n  static const f = <int>[];\n}",
                &[],
            ),
            (
                "doc_good_1",
                "const o = <int>[];\n\nclass A {\n  static const o = <int>[];\n}",
                &[],
            ),
        ];
        // The UTF-16 offset of a 1-based `(line, column)` in an ASCII source.
        fn offset(source: &str, line: u32, column: u32) -> u32 {
            let line_start: usize = source
                .split_inclusive('\n')
                .take(line as usize - 1)
                .map(str::len)
                .sum();
            (line_start + column as usize - 1) as u32
        }
        let mut mismatches = Vec::new();
        for (case, source, oracle) in cases {
            let f = Fixture::parse(source);
            let mut lints = f.run(&one_rule("prefer_const_declarations"));
            lints.sort_by_key(|lint| lint.range.start);
            let got: Vec<(u32, u32)> = lints
                .iter()
                .map(|lint| (lint.range.start, lint.range.end))
                .collect();
            let mut want: Vec<(u32, u32)> = oracle
                .iter()
                .map(|&(line, column, length)| {
                    let start = offset(source, line, column);
                    (start, start + length)
                })
                .collect();
            want.sort_unstable();
            if got != want {
                mismatches.push(format!("{case}: want {want:?} got {got:?}"));
            }
        }
        assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
    }
}
