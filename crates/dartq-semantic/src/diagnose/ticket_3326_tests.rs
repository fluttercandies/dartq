//! The #3326 pins — the unused_declaration cluster's annotation gate
//! (`Engine::finalize_usage`'s `pragma_spared` / `js_spared`).
//!
//! Upstream's `unused_local_elements_verifier` honours exactly two metadata
//! annotations: `_hasPragmaVmEntryPoint` (`@pragma('vm:entry-point')`,
//! :1179-1181, read by `_isUsedElement` :971 and `_isUsedMember` :981) and,
//! for the class element alone, `ElementMetadata.hasJS` (`@JS`,
//! `visitClassDeclaration` :82-93 → element.dart:8572-8581). Every other
//! annotation spares nothing, so a private declaration that only carries
//! `@visibleForTesting` or a `@Deprecated.*` marker still reports:
//!
//! ```text
//! @visibleForTesting
//! int _a = 0;
//!     ^ reports unused_element
//! ```
//!
//! The shapes below are the corpus population the gate decides
//! (`invalid_visibility_annotation`, `invalid_deprecated*_annotation`,
//! `invalid_widget_preview_application` — all reporting — plus
//! `unused_element`'s `test_class_isUsed_jsAnnotation`,
//! `test_method_isUsed_hasPragma_vmEntryPoint` and
//! `test_topLevelFunction_isUsed_hasPragma_vmEntryPoint` — all silent), and
//! the silent negatives that keep an unannotated private declaration
//! reporting.

use super::test_rig::{CORE_PRELUDE, report_ops};
use super::*;

/// The `(offset, length)` rows of `code` in `source`.
fn rows_with(source: &str, code: &str) -> Vec<(u32, u32)> {
    report_ops(source)
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| (d.range.offset, d.range.length))
        .collect()
}

/// The `(offset, length)` of the `nth` (0-based) occurrence of `name`,
/// shifted by the rig's prelude.
fn nth(source: &str, name: &str, index: usize) -> (u32, u32) {
    let mut from = 0usize;
    let mut offset = 0usize;
    for _ in 0..=index {
        offset = source[from..].find(name).expect("name occurrence") + from;
        from = offset + name.len();
    }
    let prelude = CORE_PRELUDE.len() as u32;
    (offset as u32 + prelude, name.len() as u32)
}

/// The load-bearing shape: a `@visibleForTesting`-annotated private
/// top-level variable reports — the marker is not a use.
#[test]
fn annotated_private_top_level_variable_reports() {
    let source = "\
@visibleForTesting
int _a = 0;
";
    assert_eq!(
        rows_with(source, "unused_element"),
        vec![nth(source, "_a", 0)],
        "{source:?}"
    );
}

/// A `@Deprecated`-annotated private class reports for the same reason.
#[test]
fn annotated_private_class_reports() {
    let source = "\
@Deprecated('gone')
class _C {}
";
    assert_eq!(
        rows_with(source, "unused_element"),
        vec![nth(source, "_C", 0)],
        "{source:?}"
    );
}

/// An annotated private method reports (`_isUsedMember` reads the pragma
/// gate alone, never `@visibleForTesting`).
#[test]
fn annotated_private_method_reports() {
    let source = "\
class A {
  @visibleForTesting
  void _m() {}
}
";
    assert_eq!(
        rows_with(source, "unused_element"),
        vec![nth(source, "_m", 0)],
        "{source:?}"
    );
}

/// An annotated private field reports: `_visitFieldElement` routes through
/// `_isReadMember`, which carries no metadata gate at all.
#[test]
fn annotated_private_field_reports() {
    let source = "\
class A {
  @visibleForTesting
  int _a = 0;
}
";
    assert_eq!(
        rows_with(source, "unused_field"),
        vec![nth(source, "_a", 0)],
        "{source:?}"
    );
}

/// Even a `@pragma`-annotated field reports — the pragma gate sits on the
/// element/member faces only, not on `_isReadMember`.
#[test]
fn pragma_private_field_reports() {
    let source = "\
class A {
  @pragma('vm:entry-point')
  int _a = 0;
}
";
    assert_eq!(
        rows_with(source, "unused_field"),
        vec![nth(source, "_a", 0)],
        "{source:?}"
    );
}

/// The `vm:entry-point` pragma still spares a private method (upstream
/// `_hasPragmaVmEntryPoint`): the runtime reaches it from outside the Dart
/// element graph.
#[test]
fn pragma_private_method_spared() {
    let source = "\
class A {
  @pragma('vm:entry-point')
  void _m() {}
}
";
    assert_eq!(rows_with(source, "unused_element"), vec![], "{source:?}");
}

/// The same pragma spares a private top-level function.
#[test]
fn pragma_private_top_level_function_spared() {
    let source = "\
@pragma('vm:entry-point')
void _f() {}
";
    assert_eq!(rows_with(source, "unused_element"), vec![], "{source:?}");
}

/// `@JS()` spares the class element (`visitClassDeclaration`'s `hasJS`
/// arm) — the corpus's `test_class_isUsed_jsAnnotation` shape.
#[test]
fn js_annotated_private_class_spared() {
    let source = "\
@JS()
class _A {}
";
    assert_eq!(rows_with(source, "unused_element"), vec![], "{source:?}");
}

/// The `@JS` gate is class-only: a `@JS`-annotated mixin reports (upstream
/// never reads the metadata on the mixin face).
#[test]
fn js_annotated_private_mixin_reports() {
    let source = "\
@JS()
mixin _M {}
";
    assert_eq!(
        rows_with(source, "unused_element"),
        vec![nth(source, "_M", 0)],
        "{source:?}"
    );
}

/// The gate compares the annotation's last dotted segment, so a prefixed
/// spelling answers too (`@js.JS()` — the corpus's `package:js` import
/// shape; the rig leaves the prefix unresolved, which is exactly the
/// spelling-only reading the declaration site keeps).
#[test]
fn prefixed_js_annotation_spares_the_class() {
    let source = "\
@js.JS()
class _A {}
";
    assert_eq!(rows_with(source, "unused_element"), vec![], "{source:?}");
}

/// The silent negative the gate must not disturb: an UNANNOTATED private
/// field of an unreferenced class still reports.
#[test]
fn unannotated_private_field_still_reports() {
    let source = "\
class A {
  int _f = 0;
}
";
    assert_eq!(
        rows_with(source, "unused_field"),
        vec![nth(source, "_f", 0)],
        "{source:?}"
    );
}

/// The other silent negative: an annotated but USED private declaration
/// stays silent (the use credit, not the annotation, is what spares it).
#[test]
fn annotated_private_used_declaration_is_silent() {
    let source = "\
@visibleForTesting
int _a = 0;
int get b => _a;
";
    assert_eq!(rows_with(source, "unused_element"), vec![], "{source:?}");
}
