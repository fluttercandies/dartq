use super::{Check, NONE_TOKEN, NodeKind, RuleSpec};
use crate::rules::{is_statement_row, slice};

/// Official: `pkg/linter/lib/src/rules/use_test_throws_matchers.dart`
/// — a try statement whose single catch clause is only reached through
/// a trailing `fail(...)` should use `expect(..., throwsA(...))`
/// instead (:53-58): exactly one catch clause, a non-empty body, no
/// `finally`, and the last body statement is an invocation of the
/// test-framework `fail` (:50-52). Report at the whole statement.
///
/// Approximation (documented): the official resolves the callee to the
/// top-level `fail` of `package:test_api/src/frontend/expect.dart`
/// (`isTestInvocation`, :41-49 — `element.library.uri` +
/// `element.name`). The URI identity face stands in syntactically: the
/// unit imports a package that exports the test `fail` (`test`,
/// `test_api`, `flutter_test`), and the callee is a bare unprefixed
/// `fail` invocation (a member send's trailing access marker fails the
/// bare-name face, and a `NoArguments` send is a tear-off, not the
/// call). Over-report: a same-file or shadowing declaration of `fail`
/// still reports. Under-report: `fail` re-exported through a helper
/// package with no direct test-framework import stays silent. Message:
/// `diagnostic.g.dart` `useTestThrowsMatchers`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "use_test_throws_matchers",
    &[NodeKind::TryStatement],
    check,
);

fn check(c: &mut Check<'_>) {
    let node = c.node();
    // `node.catchClauses.length != 1` (:54): the try's direct
    // `CatchClause` children.
    let kids = c.collect_direct(node);
    if kids
        .iter()
        .filter(|&&kid| c.kind(kid) == NodeKind::CatchClause)
        .count()
        != 1
    {
        return;
    }
    // `node.finallyBlock == null` (:56): payload word 0 is the
    // `finally` token (the `TryStatementEnd` payload).
    if c.payload(node).first() != Some(&NONE_TOKEN) {
        return;
    }
    // The body block is the try's first child; `node.body.statements.
    // isEmpty` (:54) and `.last` (:55) read its statement rows — the
    // production tiling lays each statement's expression pieces beside
    // its span-only `ExpressionStatement` leaf, so the trailing send
    // sits immediately before the last statement row.
    let Some(&body) = kids.first() else {
        return;
    };
    if c.kind(body) != NodeKind::Block {
        return;
    }
    let statements = c.collect_direct(body);
    let Some(pos) = statements.iter().rposition(|&row| is_statement_row(c, row)) else {
        return;
    };
    if c.kind(statements[pos]) != NodeKind::ExpressionStatement {
        return;
    }
    if pos == 0 || c.kind(statements[pos - 1]) != NodeKind::Send {
        return;
    }
    // A member send (`x.fail()`) carries a trailing access marker; the
    // official callee is a bare top-level name (:43-46).
    if slice::access_marker(c, &statements, pos - 1, NodeKind::DotAccess).is_some() {
        return;
    }
    let Some((head, arguments)) = slice::send_parts(c, &statements, pos - 1) else {
        return;
    };
    // A `NoArguments` send is a tear-off, not an invocation (:50-52).
    if arguments.is_none() {
        return;
    }
    if c.kind(statements[head]) != NodeKind::Identifier
        || c.lexeme(c.head_token(statements[head])) != "fail"
    {
        return;
    }
    if !imports_test_package(c) {
        return;
    }
    // `rule.reportAtNode(lastBodyStatement)` (:58) — the whole
    // statement through its `;` (the corpus `[!fail('fail');!]`).
    let range = c.utf16_span(statements[pos]);
    c.report(range, &[]);
}

/// The URI identity face: the unit imports a package whose public
/// library exports the test `fail` (`package:test_api` itself, or the
/// `test` / `flutter_test` facades over it). The written URI's package
/// segment answers; a relative or non-test import never does.
fn imports_test_package(c: &Check<'_>) -> bool {
    let Some(&root) = c.path.first() else {
        return false;
    };
    // The import rows nest inside their `TopLevelDeclaration`
    // wrappers — the flat unit window carries them all.
    c.children(root).any(|row| {
        c.kind(row) == NodeKind::Import
            && crate::rules::directives::written_uri(c, row).is_some_and(|(_, uri)| {
                uri.strip_prefix("package:")
                    .and_then(|rest| rest.split('/').next())
                    .is_some_and(|package| {
                        matches!(package, "test" | "test_api" | "flutter_test")
                    })
            })
    })
}
