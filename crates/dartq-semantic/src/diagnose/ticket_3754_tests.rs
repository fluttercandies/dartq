//! #3754 — `invalid_null_aware_operator` element/map-entry convergence pins.
//!
//! The null-aware element and map-entry faces read the operand's *static
//! type*. A `?.`-shorted access (`f('a')?.length`) carries the short-circuit's
//! own nullable result, not the trailing member's declared return, so the
//! `?` stays live. Reading the un-widened member type made the operator
//! look dead (the census's EXTRA rows over the co19 Null-aware-elements
//! corpus).

use super::test_rig::{report_ops, rows};

/// The `invalid_null_aware_operator` rows of `source`, as `message @offset`.
fn null_aware_rows(source: &str) -> Vec<String> {
    rows(&report_ops(source))
        .into_iter()
        .filter(|row| row.starts_with("invalid_null_aware_operator "))
        .collect()
}

/// A `?.`-shorted element operand is potentially nullable: the trailing
/// member returns `int` while the chain reads `int?`, so the oracle keeps
/// the `?` silent. The receiver is the prelude's `String`, whose `length`
/// the prelude declares — the chain stays well-typed, so the silence rides
/// the short-circuit rather than an invalid-type propagation (the
/// `undefined_*` guard below rejects the masked shape).
#[test]
fn shorted_element_operand_stays_silent() {
    let all = rows(&report_ops(
        "String? f(String? v) => v;\n\
         main() {\n\
           var list = [?f('a')?.length, ?f(null)?.length];\n\
         }\n",
    ));
    assert!(
        all.iter()
            .all(|row| !row.starts_with("invalid_null_aware_operator ")),
        "{all:?}"
    );
    assert!(
        all.iter().all(|row| !row.starts_with("undefined_")),
        "shorted chain must be well-typed (a missing member masks the mechanism): {all:?}"
    );
}

/// The map-entry twin: the key and the value of a null-aware map entry read
/// the shorted chain's nullable type (the prelude's `String` carries
/// `length`, and the `undefined_*` guard rejects the masked shape).
#[test]
fn shorted_map_entry_operands_stay_silent() {
    let all = rows(&report_ops(
        "String? f(String? v) => v;\n\
         main() {\n\
           var map = {?f('a')?.length: ?f('a')?.length};\n\
         }\n",
    ));
    assert!(
        all.iter()
            .all(|row| !row.starts_with("invalid_null_aware_operator ")),
        "{all:?}"
    );
    assert!(
        all.iter().all(|row| !row.starts_with("undefined_")),
        "shorted chain must be well-typed (a missing member masks the mechanism): {all:?}"
    );
}

/// Negative control: a non-shorted access on a non-nullable receiver still
/// reports at the `?` token.
#[test]
fn non_shorted_element_operand_reports() {
    let found = null_aware_rows(
        "main() {\n\
           int e1 = 42;\n\
           var list = [?e1];\n\
         }\n",
    );
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(
        found[0]
            .ends_with("The element can't be null, so the null-aware operator '?' is unnecessary."),
        "{found:?}"
    );
}

/// Negative control: a nullable operand (`int?`) keeps the `?` live.
#[test]
fn nullable_element_operand_stays_silent() {
    let found = null_aware_rows(
        "main() {\n\
           int? e2 = 2 > 1 ? 2 : null;\n\
           var list = [?e2];\n\
         }\n",
    );
    assert!(found.is_empty(), "{found:?}");
}
