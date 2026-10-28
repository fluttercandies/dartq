use std::path::Component;

use super::{Check, NodeKind, RuleSpec};

/// Official: upstream Dart SDK source,
/// `pkg/linter/lib/src/rules/use_build_context_synchronously.dart` —
/// a `BuildContext` used across an async gap reports at the use
/// (`Don't use 'BuildContext's across async gaps.`), upgraded to the
/// unrelated-guard variant (`... across async gaps, guarded by an
/// unrelated 'mounted' check.`) when a `mounted` check stood in guard
/// position but resolved to the wrong association. Messages:
/// `diagnostic.g.dart` `useBuildContextSynchronouslyAsyncUse` /
/// `useBuildContextSynchronouslyWrongMounted` — one rule name, two
/// upstream codes sharing the `name:` field, so the machine output and
/// the freeze universe carry one row.
///
/// The shell is the thin consumer of the #1198 async-gap face: use
/// identification (BuildContext-typed values, the `State.context`
/// getter shape, the nullable exemption), await-gap segmentation, and
/// mounted-guard recognition are the face's per-use verdict rows,
/// mounted through [`crate::engine::Unit::async_context_uses`]; each
/// row is reported once, at its own span. The shell selects the
/// message variant and applies the two registration gates: the
/// test-directory bail (the official registration runs only outside a
/// test directory) and the same-file `BuildContext` shadow gate (the
/// face resolves `BuildContext` by name, so a same-file declaration of
/// the type breaks the element identity — the unit stays silent rather
/// than mis-judge).
///
/// The face does not model the protected-callback closure class
/// (upstream's `Future`/`Stream`/`StreamController` argument table):
/// closure-internal uses emit no rows and so never report here. The
/// closure face is the substrate's to add — the shell never re-walks
/// flow or guesses verdicts.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "use_build_context_synchronously",
    &[NodeKind::CompilationUnit],
    check,
);

const ASYNC_USE_PROBLEM: &str = "Don't use 'BuildContext's across async gaps.";
const ASYNC_USE_CORRECTION: &str = "Try rewriting the code to not use the 'BuildContext', or guard the use with a 'mounted' check.";
const WRONG_MOUNTED_PROBLEM: &str =
    "Don't use 'BuildContext's across async gaps, guarded by an unrelated 'mounted' check.";
const WRONG_MOUNTED_CORRECTION: &str = "Guard a 'State.context' use with a 'mounted' check on the State, and other BuildContext use with a 'mounted' check on the BuildContext.";

fn check(c: &mut Check<'_>) {
    let Some(rows) = c.unit.async_context_uses else {
        return;
    };
    // The registration bail: the official rule registers only outside
    // a test directory (`context.isInTestDirectory`).
    if in_test_directory(c) {
        return;
    }
    // The element-identity gate: a same-file `BuildContext` type
    // declaration breaks the face's name-based resolution.
    if declares_build_context_type(c) {
        return;
    }
    for row in rows {
        if row.guarded {
            continue;
        }
        let (problem, correction) = if row.unrelated_mounted {
            (WRONG_MOUNTED_PROBLEM, WRONG_MOUNTED_CORRECTION)
        } else {
            (ASYNC_USE_PROBLEM, ASYNC_USE_CORRECTION)
        };
        c.report_texts(row.range.clone(), problem, correction, &[]);
    }
}

/// The official test-directory face (`workspace.dart`
/// `_testDirectoryNames`): the package-relative path's first component
/// is one of the four test directory names. Without a path or package
/// facts the gate stays out (the official no-package posture).
fn in_test_directory(c: &Check<'_>) -> bool {
    let (Some(path), Some(workspace)) = (c.unit.path, c.unit.workspace) else {
        return false;
    };
    path.strip_prefix(&workspace.root)
        .ok()
        .and_then(|relative| relative.components().next())
        .is_some_and(|component| {
            matches!(
                component,
                Component::Normal(name)
                    if name == "test"
                        || name == "integration_test"
                        || name == "test_driver"
                        || name == "testing"
            )
        })
}

/// Whether the unit declares a type named `BuildContext` (the
/// element-identity gate): a type-kind row whose wrapper-level name
/// identifier carries the spelling.
fn declares_build_context_type(c: &Check<'_>) -> bool {
    const TYPE_KINDS: [NodeKind; 6] = [
        NodeKind::ClassDeclaration,
        NodeKind::MixinDeclaration,
        NodeKind::EnumDeclaration,
        NodeKind::ExtensionDeclaration,
        NodeKind::ExtensionTypeDeclaration,
        NodeKind::Typedef,
    ];
    for top in c.collect_direct(c.node()) {
        if c.kind(top) != NodeKind::TopLevelDeclaration {
            continue;
        }
        let Some(&decl) = c
            .collect_direct(top)
            .iter()
            .find(|row| TYPE_KINDS.contains(&c.kind(**row)))
        else {
            continue;
        };
        // The declaration's own name Identifier carries a
        // declaration-name context; it tiles inside the window for a
        // typedef, or as a leading sibling for the class-likes.
        let is_name = |kid: u32| {
            c.kind(kid) == NodeKind::Identifier
                && c.lexeme(c.head_token(kid)) == "BuildContext"
                && matches!(
                    c.payload(kid).first().copied(),
                    Some(ctx)
                        if ctx
                            == dartq_parser::IdentifierContext::ClassOrMixinOrExtensionDeclaration
                                as u32
                            || ctx == dartq_parser::IdentifierContext::EnumDeclaration as u32
                            || ctx == dartq_parser::IdentifierContext::TypedefDeclaration as u32
                )
        };
        if c.children(decl).any(&is_name) {
            return true;
        }
        let siblings = c.collect_direct(top);
        if let Some(pos) = siblings.iter().position(|&kid| kid == decl)
            && siblings[..pos].iter().copied().any(is_name)
        {
            return true;
        }
    }
    false
}
