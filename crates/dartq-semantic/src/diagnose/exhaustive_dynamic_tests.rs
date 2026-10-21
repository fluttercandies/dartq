//! The switch-expression exhaustiveness pins over a top-typed scrutinee
//! (the drift `switch (fullMessage![index]) { null => null, var other =>
//! castInt(other) }` shape — the index read types `dynamic` once the
//! receiver's `[]` answers): a type-free variable pattern covers the
//! whole remaining space whatever the scrutinee's static type.
//!
//! Oracle provenance (`dart analyze` 3.13.1): every shape is silent.

use super::test_rig::{assert_silent, report_ops};

#[test]
fn var_pattern_covers_a_dynamic_scrutinee_after_null() {
    let report = report_ops(
        "int? f(dynamic x) => switch (x) {\n\
           null => null,\n\
           var other => other == null ? 0 : 1,\n\
         };\n",
    );
    assert_silent(&report);
}

#[test]
fn var_pattern_covers_a_dynamic_index_read_after_null() {
    let report = report_ops(
        "int? f(List? xs, int index) => switch (xs![index]) {\n\
           null => null,\n\
           var other => other == null ? 0 : 1,\n\
         };\n",
    );
    assert_silent(&report);
}

#[test]
fn var_pattern_covers_an_object_scrutinee_after_null() {
    let report = report_ops(
        "int? f(Object? x) => switch (x) {\n\
           null => null,\n\
           var other => other == null ? 0 : 1,\n\
         };\n",
    );
    assert_silent(&report);
}
