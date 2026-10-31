//! #3159 `class_used_as_mixin` pins: the with-clause teardown resolves the
//! entry's HEAD name — `p.C` through the prefix's table, `A<int>`/`A?`
//! through the head before the type-argument group / nullable suffix — so a
//! parameterized, nullable, or prefixed plain-class entry reports the same
//! row a bare one does (upstream `error_verifier.dart`
//! `_checkForMixinInWithClause` resolves the type's ELEMENT). Every
//! expectation is a verbatim oracle 3.13.7 row.

use super::pins::report;

/// The `class_used_as_mixin` rows of `report`, as (offset, length) pairs.
fn mixin_rows(source: &str) -> Vec<(u32, u32)> {
    report(source)
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "class_used_as_mixin")
        .map(|d| (d.range.offset, d.range.length))
        .collect()
}

/// A plain class used as a mixin reports at the whole entry — the bare
/// spelling anchors the identifier.
/// Oracle `class B = A with A;` → `class_used_as_mixin` at the `with` `A`.
#[test]
fn plain_class_as_mixin_reports() {
    let source = "class A {}\nclass B = A with A;\n";
    assert_eq!(mixin_rows(source), vec![(28, 1)]);
}

/// A `mixin` (not a class) is legal — silent.
#[test]
fn real_mixin_quiet() {
    let source = "class A {}\nmixin M {}\nclass B = A with M;\n";
    assert_eq!(mixin_rows(source), Vec::<(u32, u32)>::new());
}

/// A `mixin class` is legal — silent (the `is_mixin_class` exemption).
#[test]
fn mixin_class_quiet() {
    let source = "mixin class A {}\nclass B = A with A;\n";
    assert_eq!(mixin_rows(source), Vec::<(u32, u32)>::new());
}

/// A parameterized entry reports the whole `A<int>` span — the head `A`
/// resolves through the `<` boundary (oracle: `with A<int>` → len 6 at the
/// head).
#[test]
fn generic_class_as_mixin_reports_whole_span() {
    let source = "class A<T> {}\nclass B = A with A<int>;\n";
    assert_eq!(mixin_rows(source), vec![(31, 6)]);
}

/// `A<int>` on a non-generic class still resolves the head — the arity
/// mismatch is `wrong_number_of_type_arguments`'s own face, never a reason
/// to drop the mixin row (oracle reports both).
#[test]
fn generic_args_on_nongeneric_still_reports() {
    let source = "class A {}\nclass B = A with A<int>;\n";
    assert_eq!(mixin_rows(source), vec![(28, 6)]);
}

/// A nullable entry reports the whole `A?` span — the head `A` resolves
/// through the `?` boundary.
#[test]
fn nullable_class_as_mixin_reports_whole_span() {
    let source = "class A {}\nclass B = A with A?;\n";
    assert_eq!(mixin_rows(source), vec![(28, 2)]);
}

/// A `sealed` class is still a plain class for this face — the
/// `is_mixin_class` exemption covers only `mixin`/`mixin class`, never the
/// inheritance modifiers (oracle reports `sealed`/`base`/`interface`/`final`
/// alike).
#[test]
fn sealed_class_as_mixin_reports() {
    let source = "sealed class A {}\nclass B = A with A;\n";
    assert_eq!(mixin_rows(source), vec![(35, 1)]);
}

/// `Object` is a plain class — usable in a `with` clause reports the
/// whole `Object` span (oracle: `class B = Object with Object;`).
#[test]
fn object_as_mixin_reports() {
    let source = "class B = Object with Object;\n";
    assert_eq!(mixin_rows(source), vec![(22, 6)]);
}

/// A regular `extends S with C` header applies the same gate — `C` is a
/// plain class, so it reports.
#[test]
fn extends_with_class_reports() {
    let source = "class S {}\nclass C {}\nclass B extends S with C {}\n";
    // `with C` head is at offset 45.
    assert_eq!(mixin_rows(source), vec![(45, 1)]);
}

/// Multiple entries each report their own row — `with M, A` reports `A`
/// (the plain class) while `M` (a real mixin) stays silent.
#[test]
fn mixed_entries_report_only_classes() {
    let source = "class A {}\nmixin M {}\nclass B = A with M, A;\n";
    // `A` at offset 42 is the second entry's head.
    assert_eq!(mixin_rows(source), vec![(42, 1)]);
}
