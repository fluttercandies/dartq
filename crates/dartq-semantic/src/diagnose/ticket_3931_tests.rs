//! #3931 routing pins: the `syntax_test_62` case's two semantic faces —
//! `unnecessary_type_check` on a REJECTED `is` type, and the spurious
//! `type_test_with_undefined_name` that same rejection produces.
//!
//! The case (`features/_core/syntax/language/syntax/syntax_test_62`, one
//! line `print(null is void);` at 207) diverges on both:
//!
//! ```text
//! oracle: EXPECTED_TYPE_NAME            207:19|4   Expected a type name.
//!         UNNECESSARY_TYPE_CHECK        207:11|12  ... always 'true'.
//! dartq : expected_type_name            207:19|4   Type 'void' can't be used here.
//!         type_test_with_undefined_name 207:23|1   The name ')' isn't defined ...
//! ```
//!
//! Both dartq rows share ONE cause: the written `is` type (`void`, which the
//! parser rejects — `void` is no `is`-type) never reaches a resolved face.
//!
//! * `unnecessary_type_check` — `Engine::check_type_check`
//!   (`engine_core.rs:8053`) starts with `let Some(armed) = self.armed_ty
//!   else { return };`, so a rejected is-type arms nothing and no verdict
//!   runs. Upstream keeps the check: the recovered type is `InvalidType`,
//!   and its subtype arm answers true against everything
//!   (`subtype.dart:274-276`, mirrored by `TypeSystem::is_subtype`'s 0c
//!   rule), which is why the oracle reads constant-true. The official
//!   decision face already exists in the tree and has NO semantic consumer:
//!   `dartq-types/src/provably.rs:122` `type_check_verdict`.
//! * `type_test_with_undefined_name` — the listener latches the is-type
//!   mark at `listener.rs:7271` and `resolution.rs:4946`
//!   (`emit_is_type_miss`) reports the mark as an undefined NAME, landing on
//!   the token after the rejected type (`)`). Upstream reports the
//!   `expected_type_name` syntax error only; an invalid type is no
//!   undefined name.
//!
//! Routing: the fix points are outside this packet's scope (`engine_core.rs`
//! and `listener.rs` belong to the basis-lane roster; `resolution.rs` to the
//! unresolved-finalization lane). The two `#[ignore]`d pins below state the
//! oracle-verified expectation so the owning lane only has to drop the
//! attribute.

use super::test_rig::report_ops;

const TYPE_CHECK: &str = "unnecessary_type_check";
const UNDEFINED_NAME: &str = "type_test_with_undefined_name";

/// The `code` rows of `report`, as messages in row order.
fn messages(report: &crate::diagnose::FileReport, code: &str) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|row| row.code.name == code)
        .map(|row| row.message.to_string())
        .collect()
}

/// The control: a resolved `is` between two known types still decides the
/// check — the verdict walk that the two routing pins below need to reach.
#[test]
fn resolved_type_check_reports_constant_true() {
    let source = "void f() {\n  print(1 is int);\n}\n";
    assert_eq!(
        messages(&report_ops(source), TYPE_CHECK),
        vec!["Unnecessary type check; the result is always 'true'.".to_string()]
    );
}

/// A rejected `is` type (`void` is no `is`-type, so the parser reports
/// `expected_type_name` and recovers to an invalid type) still decides the
/// check: the invalid face is a subtype of everything, so the oracle reads
/// constant-true.
///
/// Route: `engine_core.rs` `Engine::check_type_check`'s `armed_ty` gate
/// (`:8053`) and the listener's armed-type capture; the official decision
/// already exists unused at `dartq-types/src/provably.rs:122`.
#[test]
#[ignore = "routed: engine_core.rs check_type_check armed_ty gate (:8053) + \
            listener.rs armed capture; official face unused at \
            dartq-types/src/provably.rs:122"]
fn rejected_is_type_still_decides_the_check() {
    let source = "void f() {\n  print(null is void);\n}\n";
    assert_eq!(
        messages(&report_ops(source), TYPE_CHECK),
        vec!["Unnecessary type check; the result is always 'true'.".to_string()]
    );
}

/// The same rejection must NOT surface as an undefined name: upstream reports
/// only the `expected_type_name` syntax error for `null is void`.
///
/// Route: `listener.rs:7271` (the is-type mark) and `resolution.rs:4946`
/// (`emit_is_type_miss`), which report the token after the rejected type.
#[test]
#[ignore = "routed: listener.rs:7271 is_type_mark + resolution.rs:4946 \
            emit_is_type_miss report the token after a rejected is-type"]
fn rejected_is_type_reports_no_undefined_name() {
    let source = "void f() {\n  print(null is void);\n}\n";
    assert!(
        messages(&report_ops(source), UNDEFINED_NAME).is_empty(),
        "a rejected is-type is no undefined name"
    );
}
