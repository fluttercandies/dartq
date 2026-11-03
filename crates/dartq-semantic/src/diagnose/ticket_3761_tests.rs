//! The #3761 pins: cross-case diagnostic leakage through the verdict-side
//! alias fold's graph-wide unique-typedef chase.
//!
//! A private, unexported typedef that a foreign member signature spells
//! (co19's `_CheckExceptionFn` — the `Expect.throws` `check` formal) never
//! enters the use site's import scope, so `fold_alias_verdict` resolves its
//! right-hand side by sweeping every unit of the graph for a unique
//! declaration. The conformance harness packs many same-named package roots
//! into ONE resolved graph — every `library expect` copy declares its own
//! `_CheckExceptionFn` — and the chase's original first-hit/ambiguous rule
//! declined to fold as soon as a second copy appeared. The unfolded head
//! fell into `check_arguments`' non-function-formal refusal arm and
//! fabricated `argument_type_not_assignable` rows that a single-root run
//! never emits (case `static_analysis_external_A01_t03` emitted four rows
//! only when `static_analysis_abstract_A06_t03`'s copy of `library expect`
//! shared the graph).
//!
//! # The contract
//!
//! - Every graph unit declaring the same private typedef spelling with a
//!   captured right-hand side that ALL project to one identical type folds
//!   to that type — identical declarations across mutually invisible
//!   libraries are one denotation, not an ambiguity.
//! - A genuinely divergent candidate set (a differing right-hand side, or a
//!   declaration whose right-hand side was never captured) still stays
//!   unfolded — the under-report direction, never a fabricated fold.

use crate::diagnose::test_rig::{RigImport, assert_silent, code_names, report_ops_imports, rows};

/// The `Expect.throws` face both fixture libraries share: `check` spells
/// the private typedef exactly as co19's `expect_common.dart` does. The
/// typedef's `dynamic` parameter stands in for the corpus's untyped
/// `exception` formal.
const EXPECT_LIB: &str = "typedef bool _CheckExceptionFn(dynamic e);\n\
     class Expect {\n\
       static void throws(void f(), [_CheckExceptionFn? check]) {}\n\
     }\n";

/// The use site every poisoned case wrote: a `void Function()` thunk and a
/// `(e) => e is …` closure against the `_CheckExceptionFn?` formal.
const USE_SITE: &str = "void main() {\n  Expect.throws(() {}, (e) => e is Exception);\n}\n";

/// One declaring library in the graph: the alias's right-hand side folds
/// and the use site stays silent (the single-root baseline).
#[test]
fn single_private_typedef_decl_folds_and_stays_silent() {
    let report = report_ops_imports(
        USE_SITE,
        &[RigImport {
            uri: "package:t/expect_a.dart",
            prefix: None,
            source: EXPECT_LIB,
        }],
    );
    assert_silent(&report);
}

/// Two declaring libraries with byte-identical right-hand sides — the
/// conformance harness's `library expect` copies. The second copy lives in
/// a library the use site cannot name the typedef from (the chase is
/// graph-wide, and the pinned face is what it answers): the fold must see
/// one denotation, not an ambiguity. Pre-#3761 the refusal arm emitted
/// `argument_type_not_assignable` at the closure argument; post-fix the
/// identical right-hand sides fold and the site stays silent. The `Marker`
/// read keeps the second import exercised.
#[test]
fn duplicate_private_typedef_decls_identical_rhs_stay_silent() {
    let report = report_ops_imports(
        "void main() {\n  Expect.throws(() {}, (e) => e is Exception);\n  Marker();\n}\n",
        &[
            RigImport {
                uri: "package:t/expect_a.dart",
                prefix: None,
                source: EXPECT_LIB,
            },
            RigImport {
                uri: "package:t/expect_b.dart",
                prefix: None,
                source: "typedef bool _CheckExceptionFn(dynamic e);\nclass Marker {}\n",
            },
        ],
    );
    assert_silent(&report);
}

/// The divergent-candidate boundary: a second library whose same-spelled
/// typedef captures a DIFFERENT right-hand side must not fold — the use
/// site keeps the under-report posture of a genuinely ambiguous private
/// spelling rather than picking a winner. What this pair emits is the
/// refusal row itself, the documented under-report of an undecidable fold;
/// the pin fixes the direction so a future widening is deliberate.
#[test]
fn divergent_private_typedef_decls_decline_the_fold() {
    let report = report_ops_imports(
        "void main() {\n  Expect.throws(() {}, (e) => e is Exception);\n  Marker();\n}\n",
        &[
            RigImport {
                uri: "package:t/expect_a.dart",
                prefix: None,
                source: EXPECT_LIB,
            },
            RigImport {
                uri: "package:t/expect_b.dart",
                prefix: None,
                source: "typedef int _CheckExceptionFn(dynamic e, dynamic f);\nclass Marker {}\n",
            },
        ],
    );
    // Oracle is silent here (a single root cannot see both private copies); dartq's under-report is tracked in #3761.
    assert_eq!(
        code_names(&report),
        vec!["argument_type_not_assignable"],
        "{:?}",
        rows(&report)
    );
}
