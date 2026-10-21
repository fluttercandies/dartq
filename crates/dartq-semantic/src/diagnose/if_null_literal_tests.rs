//! The if-null assignment's promotion face over a literal right side (the
//! `collection` corpus shape `(chunk ??= []).add(element); … yield chunk;`):
//! `x ??= e` leaves `x` promoted to the non-null declared type when `e`
//! is non-null, and a collection literal is non-null whatever its inferred
//! arguments — the `yield` of the promoted local satisfies the generator's
//! element type.
//!
//! Oracle provenance (`dart analyze` 3.13.1): both shapes are silent.

use super::test_rig::{assert_silent, report_ops};

#[test]
fn if_null_assign_of_a_creation_promotes_for_the_yield() {
    let report = report_ops(
        "class Chunk { void add(int e) {} }\n\
         Iterable<Chunk> f(List<int> xs, bool Function(int) test) sync* {\n\
           Chunk? chunk;\n\
           for (var element in xs) {\n\
             (chunk ??= Chunk()).add(element);\n\
             if (test(element)) {\n\
               yield chunk;\n\
               chunk = null;\n\
             }\n\
           }\n\
         }\n",
    );
    assert_silent(&report);
}

#[test]
fn if_null_assign_of_a_list_literal_promotes_for_the_yield() {
    let report = report_ops(
        "Iterable<List<int>> f(List<int> xs, bool Function(int) test) sync* {\n\
           List<int>? chunk;\n\
           for (var element in xs) {\n\
             (chunk ??= []).add(element);\n\
             if (test(element)) {\n\
               yield chunk;\n\
               chunk = null;\n\
             }\n\
           }\n\
         }\n",
    );
    assert_silent(&report);
}
