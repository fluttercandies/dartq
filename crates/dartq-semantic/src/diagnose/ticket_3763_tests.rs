//! #3763 — `unused_element` extras: the unnamed extension's `this` face.
//!
//! The oracle resolves `this` inside an extension body to the `on` type
//! (`ExtensionElement.extendedType`), so `this.p ??= v` credits the
//! property's getter AND setter (`_addMemberAndCorrespondingGetter`,
//! upstream Dart SDK source, unused_local_elements_verifier.dart:301-348)
//! and the pair never reports. This engine's own resolution kept the
//! unnamed face's placeholder container instead of the `on` head — the
//! accessor pair then credited nothing and both declarations reported.
//!
//! The sibling shapes pin the boundary: a pure write credits the SETTER
//! element alone (the getter still reports), and a never-touched accessor
//! pair reports on both faces.

use super::test_rig::{report_ops, rows};

/// The `unused_element` rows of `source` that name `member`.
fn unused_member(source: &str, member: &str) -> Vec<String> {
    let quoted = format!("'{member}'");
    rows(&report_ops(source))
        .into_iter()
        .filter(|row| row.starts_with("unused_element ") && row.contains(&quoted))
        .collect()
}

/// `this.p ??= v` inside an UNNAMED extension credits the accessor pair —
/// neither the getter nor the setter reports.
#[test]
fn this_null_assign_in_unnamed_extension_spares_accessor_pair() {
    let found = unused_member(
        "extension on String {\n\
         \x20 int? get a1 => null;\n\
         \x20 set a1(Object? value) {}\n\
         \x20 void t() {\n\
         \x20   this.a1 ??= 1;\n\
         \x20 }\n\
         }\n",
        "a1",
    );
    assert!(found.is_empty(), "{found:?}");
}

/// The same shape in a NAMED extension keeps its own credit (the named
/// face never rode the placeholder container).
#[test]
fn this_null_assign_in_named_extension_spares_accessor_pair() {
    let found = unused_member(
        "extension E on String {\n\
         \x20 int? get a1 => null;\n\
         \x20 set a1(Object? value) {}\n\
         \x20 void t() {\n\
         \x20   this.a1 ??= 1;\n\
         \x20 }\n\
         }\n",
        "a1",
    );
    assert!(found.is_empty(), "{found:?}");
}

/// A plain `this.p = v` resolves the SETTER element alone
/// (`writeOrReadElement`): the setter is spared, the getter still reports.
#[test]
fn this_plain_write_spares_setter_only() {
    let found = unused_member(
        "extension on String {\n\
         \x20 int? get b2 => null;\n\
         \x20 set b2(Object? value) {}\n\
         \x20 void t() {\n\
         \x20   this.b2 = 2;\n\
         \x20 }\n\
         }\n",
        "b2",
    );
    assert_eq!(found.len(), 1, "{found:?}");
}

/// A never-touched accessor pair still reports both faces — the `this`
/// credit never blanket-spares an extension member.
#[test]
fn untouched_accessor_pair_still_reports_both_faces() {
    let found = unused_member(
        "extension on String {\n\
         \x20 int? get c3 => null;\n\
         \x20 set c3(Object? value) {}\n\
         }\n",
        "c3",
    );
    assert_eq!(found.len(), 2, "{found:?}");
}
