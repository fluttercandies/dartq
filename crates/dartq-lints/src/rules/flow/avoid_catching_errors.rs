use dartq_ast::NodeKind;

use super::{Check, RuleSpec};

/// The dart:core `Error` family by name (sdk/lib/core/errors.dart):
/// `Error` itself plus its public subtypes. The official asks the
/// type system (`exceptionType.implementsInterface('Error',
/// 'dart.core')`, :41).
const ERROR_FAMILY: [&str; 13] = [
    "Error",
    "ArgumentError",
    "AssertionError",
    "ConcurrentModificationError",
    "IndexError",
    "NoSuchMethodError",
    "OutOfMemoryError",
    "RangeError",
    "StackOverflowError",
    "StateError",
    "TypeError",
    "UnimplementedError",
    "UnsupportedError",
];

const CLASS_PROBLEM: &str = "The type 'Error' should not be caught.";
const SUBCLASS_PROBLEM: &str =
    "The type '{0}' should not be caught because it is a subclass of 'Error'.";
const CORRECTION: &str = "Try removing the catch or catching an 'Exception' instead.";

/// Official: `upstream Dart SDK source,
/// pkg/linter/lib/src/rules/avoid_catching_errors.dart` — a catch
/// clause whose `on` type implements dart:core `Error` reports at
/// the clause (:39-52): the exact `Error` type with the class code,
/// any subtype with the subclass code (the written type spelled into
/// `{0}`); a clause without an `on` type never enters.
///
/// Approximation (documented, class R2): the subtype verdict is a
/// spelling allowlist — the dart:core `Error` family by name.
/// Under-reports: user `extends Error` hierarchies and prefixed or
/// aliased spellings (the type-system faces the allowlist cannot
/// see). Over-reports: user classes named like a family member
/// without implementing `Error` (pinned by test). Message:
/// `diagnostic.g.dart` `avoidCatchingErrorsClass` /
/// `avoidCatchingErrorsSubclass`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "avoid_catching_errors",
    &[NodeKind::CatchClause],
    check,
);

fn check(c: &mut Check<'_>) {
    let node = c.node();
    // `node.exceptionType` (:40) — the `on` form only; the head token
    // is the `on` keyword when present (head-when-present).
    if c.lexeme(c.head_token(node)) != "on" {
        return;
    }
    // The written type: the `Type` child after the `on` keyword
    // (`compute_type`, parser try-statement lane).
    let Some(type_row) = c
        .collect_direct(node)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::Type)
    else {
        return;
    };
    let name = c.lexeme(c.head_token(type_row));
    if !ERROR_FAMILY.contains(&name) {
        return;
    }
    // The whole clause is the reported node (:43, :45-50); the two
    // official codes split on the exact-`Error` face, the written
    // name filling the subclass `{0}`.
    let range = c.utf16_span(node);
    if name == "Error" {
        c.report_texts(range, CLASS_PROBLEM, CORRECTION, &[]);
    } else {
        c.report_texts(range, SUBCLASS_PROBLEM, CORRECTION, &[name]);
    }
}

#[cfg(test)]
mod tests {
    use crate::testsupport::{Fixture, one_rule, tok, utf16_at};

    fn run(source: &str) -> Vec<crate::engine::Lint> {
        Fixture::parse(source).run(&one_rule("avoid_catching_errors"))
    }

    /// `test_exactlyError` — the class code, message verbatim, the
    /// report pinning the whole clause.
    #[test]
    fn on_error_reports_the_class_message() {
        let source = "void f() {\n  try {} on Error catch (_) {}\n}\n";
        let f = Fixture::parse(source);
        let lints = f.run(&one_rule("avoid_catching_errors"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(lints[0].message, "The type 'Error' should not be caught.");
        assert_eq!(
            lints[0].correction.as_deref(),
            Some("Try removing the catch or catching an 'Exception' instead.")
        );
        assert_eq!(
            lints[0].range.start,
            utf16_at(&f, tok(source, &f.tokens, "on", 0))
        );
    }

    /// The subclass code with the written type spelled into `{0}`.
    #[test]
    fn on_error_subtype_reports_the_subclass_message() {
        let lints = run("void f() {\n  try {} on StateError catch (_) {}\n}\n");
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "The type 'StateError' should not be caught because it is a subclass of 'Error'."
        );
    }

    /// Family members beyond the `Error` head all report.
    #[test]
    fn family_members_report() {
        for name in [
            "ArgumentError",
            "AssertionError",
            "ConcurrentModificationError",
            "IndexError",
            "NoSuchMethodError",
            "OutOfMemoryError",
            "RangeError",
            "StackOverflowError",
            "StateError",
            "TypeError",
            "UnimplementedError",
            "UnsupportedError",
        ] {
            let source = format!("void f() {{\n  try {{}} on {name} catch (_) {{}}\n}}\n");
            assert_eq!(run(&source).len(), 1, "{name}");
        }
    }

    /// A bare `catch` has no exception type (:40) — silent.
    #[test]
    fn bare_catch_is_silent() {
        assert!(run("void f() {\n  try {} catch (e) {}\n}\n").is_empty());
    }

    /// `test_exactlyException` — `Exception` is not the `Error`
    /// family.
    #[test]
    fn on_exception_is_silent() {
        assert!(run("void f() {\n  try {} on Exception catch (_) {}\n}\n").is_empty());
    }

    /// Any non-family type stays silent.
    #[test]
    fn on_other_type_is_silent() {
        assert!(run("void f() {\n  try {} on String catch (_) {}\n}\n").is_empty());
    }

    /// The pinned over-report face: a user class named like a family
    /// member without implementing `Error` still reports (the
    /// allowlist is name-level).
    #[test]
    fn user_same_name_still_reports() {
        let source =
            "class StateError {}\n\nvoid f() {\n  try {} on StateError catch (_) {}\n}\n";
        assert_eq!(run(source).len(), 1);
    }

    /// The pinned under-report face: a user `extends Error` hierarchy
    /// outside the allowlist stays silent.
    #[test]
    fn unknown_error_subtype_under_reports() {
        let source = "class MyError extends Error {}\n\nvoid f() {\n  try {} on MyError catch (_) {}\n}\n";
        assert!(run(source).is_empty());
    }

    /// A prefixed spelling's head is the prefix — under-report.
    #[test]
    fn prefixed_spelling_is_silent() {
        assert!(
            run("import 'dart:io' as io;\n\nvoid f() {\n  try {} on io.SocketException catch (_) {}\n}\n")
                .is_empty()
        );
    }
}
