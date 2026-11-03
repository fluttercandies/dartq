use super::{Check, NodeKind, RuleSpec};
use crate::rules::{const_keyword, in_const_context};

/// Official: `pkg/linter/lib/src/rules/unnecessary_const.dart:38-77` — a
/// `const` keyword on a literal / instance creation already inside a
/// constant context reports at the keyword.
///
/// The context face is the official `constantContext` walk (ported as
/// [`crate::rules::in_const_context`] — the rule declares
/// `canUseParsedResult`, so the walk is the whole story). The visitor's
/// `ConstantPattern` exemptions are structural with our payload layout: a
/// constant pattern owns the `const` keyword itself (`ConstantPattern`
/// head-when-present), so the wrapped expression
/// carries none and never dispatches. Dot-shorthand const invocations
/// (`case const .new()`) are likewise pattern-owned and skip.
///
/// Dispatch keys the official node faces: the official `TypedLiteral`
/// tiles as a payload-carrying literal row (`ConstKeyword`) under a
/// keyword-less `ConstLiteral` shell whose window starts at the bracket —
/// the shell's head is never `const`, so it dispatches nothing and the
/// literal row carries the whole verdict. Message:
/// `diagnostic.g.dart` `unnecessaryConst`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "unnecessary_const",
    &[
        NodeKind::ConstExpression,
        NodeKind::ConstDotShorthand,
        NodeKind::ListLiteral,
        NodeKind::SetOrMapLiteral,
        NodeKind::RecordLiteral,
    ],
    check,
);

fn check(c: &mut Check<'_>) {
    let node = c.node();
    // The dot-shorthand const invocation (`const .new()`) opens its
    // `ConstDotShorthand` row at the `const` keyword itself
    // (`visitDotShorthandConstructorInvocation` reports at
    // `node.constKeyword`), which the shared `const_keyword` table does
    // not key — read the head token directly.
    let keyword = if c.kind(node) == NodeKind::ConstDotShorthand {
        let head = c.head_token(node);
        if c.lexeme(head) != "const" {
            return;
        }
        head
    } else {
        // `keyword == null || keyword.type != CONST` bails (:57-59).
        let Some(keyword) = const_keyword(c, node) else {
            return;
        };
        keyword
    };
    // `node.inConstantContext` (:60-62).
    if !in_const_context(c) {
        return;
    }
    // `rule.reportAtToken(constKeyword)` reports the keyword's whole
    // token (the official `unnecessaryConst` extent — `const`, 5 wide).
    let span = crate::rules::utf16_token_span(c, keyword);
    c.report(span.start..span.end, &[]);
}

#[cfg(test)]
mod tests {
    use crate::testsupport::{Fixture, one_rule};

    /// The whole upstream oracle corpus, pinned case-for-case: the
    /// 12 `UnnecessaryConstPatternsTest` + 26 `UnnecessaryConstRecordsTest`
    /// extraction cases (`pkg/linter/test/rules/unnecessary_const_test.dart`)
    /// plus the 4 `messages.yaml` doc cases — 42 total,
    /// `conformance/status.tsv` `lint unnecessary_const 42 …`. Each entry is
    /// the exact materialized case source (test-file cases carry a leading
    /// blank line, the doc files do not) and the oracle's
    /// `(line, column, length)` for every `unnecessary_const` row
    /// (`expected.tsv`). Only the rule's own rows are pinned — the
    /// co-reported `unused_local_variable` rows in the doc cases are other
    /// faces. The switch-`case const …` shapes stay silent: the
    /// `ConstantPattern` owns the keyword, so the wrapped expression never
    /// dispatches; `const A(const []);` as a bare statement stays silent
    /// because no ancestor opens a constant context.
    ///
    /// Every mismatch is collected before the single panic so one run shows
    /// the whole deviation set.
    #[test]
    fn unnecessary_const_oracle_corpus_pins() {
        // (case, source, oracle rows as (line, column, length)).
        let cases: &[(&str, &str, &[(u32, u32, u32)])] = &[
            (
                "patterns/case_constConstructor_dotShorthand_named_ok",
                "\nclass C {\n  const C.named();\n}\nf(C c) {\n  switch (c) {\n    case const .named():\n  }\n}\n",
                &[],
            ),
            (
                "patterns/case_constConstructor_dotShorthand_ok",
                "\nclass C {\n  const C();\n}\nf(C c) {\n  switch (c) {\n    case const .new():\n  }\n}\n",
                &[],
            ),
            (
                "patterns/case_constConstructor_ok",
                "\nclass C {\n  const C();\n}\nf(C c) {\n  switch (c) {\n    case const C():\n  }\n}\n",
                &[],
            ),
            (
                "patterns/case_listLiteral_ok",
                "\nvoid f(Object o) {\n  switch (o) {\n    case const [1, 2]:\n  }\n}\n",
                &[],
            ),
            (
                "patterns/case_mapLiteral_ok",
                "\nvoid f(Object o) {\n  switch (o) {\n   case const {'k': 'v'}:\n  }\n}\n",
                &[],
            ),
            (
                "patterns/case_setLiteral_ok",
                "\nvoid f(Object o) {\n  switch (o) {\n    case const {1}:\n  }\n}\n",
                &[],
            ),
            (
                "patterns/constConstructor",
                "\nclass C {\n  const C();\n}\nconst c = const C();\n",
                &[(5, 11, 5)],
            ),
            (
                "patterns/constConstructor_dotShorthand",
                "\nclass C {\n  const C();\n}\nconst C c = const .new();\n",
                &[(5, 13, 5)],
            ),
            (
                "patterns/constConstructor_dotShorthand_named",
                "\nclass C {\n  const C.named();\n}\nconst C c = const .named();\n",
                &[(5, 13, 5)],
            ),
            (
                "patterns/listLiteral",
                "\nconst l = const [];\n",
                &[(2, 11, 5)],
            ),
            (
                "patterns/mapLiteral",
                "\nconst m = const {1: 1};\n",
                &[(2, 11, 5)],
            ),
            (
                "patterns/setLiteral",
                "\nconst s = const {1};\n",
                &[(2, 11, 5)],
            ),
            (
                "records/constCall_nonConstArgument",
                "\nvar x = const A({});\nclass A {\n  const A(Object o);\n}\n",
                &[],
            ),
            (
                "records/constVariable_constCall",
                "\nconst x = const A();\nclass A {\n  const A();\n}\n",
                &[(2, 11, 5)],
            ),
            (
                "records/constVariable_constCall_newName_noArgument",
                "\nconst x = const A.new();\nclass A {\n  const A();\n}\n",
                &[(2, 11, 5)],
            ),
            (
                "records/constVariable_constCall_nonConstArgument",
                "\nconst x = const A([]);\nclass A {\n  const A(Object o);\n}\n",
                &[(2, 11, 5)],
            ),
            (
                "records/constVariable_nonConstCall",
                "\nconst x = A();\nclass A {\n  const A();\n}\n",
                &[],
            ),
            (
                "records/constVariable_nonConstCall_nonConstArgument",
                "\nconst x = A([]);\nclass A {\n  const A(Object o);\n}\n",
                &[],
            ),
            (
                "records/noContext_constCall",
                "\nvoid f() {\n  const A();\n}\nclass A {\n  const A();\n}\n",
                &[],
            ),
            (
                "records/noContext_constCall_newName",
                "\nvoid f() {\n  const A.new();\n}\nclass A {\n  const A();\n}\n",
                &[],
            ),
            (
                "records/noContext_constCall_nonConstArgument",
                "\nvoid f() {\n  const A(A());\n}\nclass A {\n  const A([o]);\n}\n",
                &[],
            ),
            (
                "records/noContext_constCall_nonConstListArgument",
                "\nvoid f() {\n  const A([]);\n}\nclass A {\n  const A(Object o);\n}\n",
                &[],
            ),
            (
                "records/noContext_newName_nonConstCall",
                "\nvoid f() {\n  A.new();\n}\nclass A {\n  const A();\n}\n",
                &[],
            ),
            (
                "records/noContext_nonConstCall",
                "\nvoid f() {\n  A();\n}\nclass A {\n  const A();\n}\n",
                &[],
            ),
            (
                "records/noContext_nonConstCall_constListArgument",
                "\nvoid f() {\n  A(const []);\n}\nclass A {\n  const A([o]);\n}\n",
                &[],
            ),
            (
                "records/noContext_nonConstCall_constObjectArgument",
                "\nvoid f() {\n  A(const A());\n}\nclass A {\n  const A([o]);\n}\n",
                &[],
            ),
            (
                "records/noContext_nonConstCall_nonConstArgument",
                "\nvoid f() {\n  A([]);\n}\nclass A {\n  const A([o]);\n}\n",
                &[],
            ),
            (
                "records/noContext_nonConstCall_nonConstObjectArgument",
                "\nvoid f() {\n  A(A());\n}\nclass A {\n  const A([o]);\n}\n",
                &[],
            ),
            (
                "records/recordLiteral",
                "\nconst r = const (a: 1);\n",
                &[(2, 11, 5)],
            ),
            ("records/recordLiteral_ok", "\nconst r = (a: 1);\n", &[]),
            (
                "records/variable_constCall_constListArgument",
                "\nvar x = const A(const []);\nclass A {\n  const A(Object o);\n}\n",
                &[(2, 17, 5)],
            ),
            (
                "records/variable_constCall_constSetArgument",
                "\nvar x = const A(const {});\nclass A {\n  const A(Object o);\n}\n",
                &[(2, 17, 5)],
            ),
            (
                "records/variable_constCall_newName_constArgument",
                "\nvar x = const A.new(const []);\nclass A {\n  const A(Object o);\n}\n",
                &[(2, 21, 5)],
            ),
            (
                "records/variable_constCall_nonConstArgument",
                "\nfinal x = const A([]);\nclass A {\n  const A(Object o);\n}\n",
                &[],
            ),
            (
                "records/variable_constCall_nonConstListArgument",
                "\nvar x = const A([]);\nclass A {\n  const A(Object o);\n}\n",
                &[],
            ),
            (
                "records/variable_nonConstCall",
                "\nvar a = A();\nclass A {\n  const A();\n}\n",
                &[],
            ),
            (
                "records/variable_nonConstCall_constListArgument",
                "\nvar x = A(const []);\nclass A {\n  const A(Object o);\n}\n",
                &[],
            ),
            (
                "records/variable_nonConstCall_newName_constArgument",
                "\nvar x = A.new(const []);\nclass A {\n  const A(Object o);\n}\n",
                &[],
            ),
            ("doc_bad_0", "const l = const <int>[];\n", &[(1, 11, 5)]),
            (
                "doc_bad_1",
                "class A { const A(); }\nm(){\n  const a = const A();\n  final b = const [const A()];\n}\n",
                &[(3, 13, 5), (4, 20, 5)],
            ),
            ("doc_good_0", "const l = <int>[];\n", &[]),
            (
                "doc_good_1",
                "class A { const A(); }\nm(){\n  const a = A();\n  final b = const [A()];\n}\n",
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
            let mut lints = f.run(&one_rule("unnecessary_const"));
            lints.sort_by_key(|lint| lint.range.start);
            let got: Vec<(u32, u32)> = lints
                .iter()
                .map(|lint| (lint.range.start, lint.range.end))
                .collect();
            let want: Vec<(u32, u32)> = oracle
                .iter()
                .map(|&(line, column, length)| {
                    let start = offset(source, line, column);
                    (start, start + length)
                })
                .collect();
            if got != want {
                mismatches.push(format!("{case}: want {want:?} got {got:?}"));
            }
        }
        assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
    }
}
