//! #4294 pins: the `is!`-guard + early-return continuation sees the
//! PROMOTED type when the tested head is a bare name two reachable files
//! declare (a first-writer-wins registry slot, not the unit's own
//! resolution, decides it — the cockpit corpus's `dart:ui`-reachable
//! `Image` preempting `package:flutter`'s).
//!
//! The promotion step is the one gate where a wrong attribution
//! manufactures a report: a refused step leaves the declared type in
//! place, and every member the guard was there to make legal reports
//! `undefined_getter` — six oracle-proven false positives on
//! cockpit-cas-b, all `widget.width`/`height`/`fit`/`repeat` after
//! `if (widget is! Image) { return …; }`. The gate answers name-level over
//! such heads ([`dartq_semantic::resolved_ops`]'s `ambiguous_names`), the
//! same posture the lossy-overload set already takes for PREFIXED
//! spellings.
//!
//! Oracle provenance: `dart analyze` 3.13.7 over every shape below —
//! `is!` guards exited by `return`/`throw`/`break`/`continue`, the
//! positive `is`-guard control, local and parameter receivers, nullable
//! declared types, and the literal-element position — reports no issues
//! on all of them; the shapes that stay unpromoted (a single-declaration
//! unrelated head, no guard at all) report the same `undefined_getter`
//! rows dartq reports.

use super::FileReport;
use super::test_rig::{RigImport, report_ops, report_ops_bare, report_ops_imports};

/// The messages of one code's rows, in source order.
fn texts(report: &FileReport, code: &str) -> Vec<String> {
    let mut rows = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .collect::<Vec<_>>();
    rows.sort_by_key(|d| d.range.offset);
    rows.iter().map(|d| d.message.to_string()).collect()
}

/// The collided declaration set: `ui.dart` first in the import list, so
/// the unrelated `Image` claims the bare registry slot first (the
/// corpus's registration order) and `decls.dart`'s subtype-of-`Widget`
/// `Image` loses it — first-writer-wins.
const UI_IMPORT: RigImport<'_> = RigImport {
    uri: "package:t/ui.dart",
    prefix: None,
    source: "class Image { int get width => 0; int get height => 0; }",
};

const DECLS_IMPORT: RigImport<'_> = RigImport {
    uri: "package:t/decls.dart",
    prefix: None,
    source: "class Widget {}\n\n\
             class Image extends Widget {\n\
             \x20 double? get width => 1;\n\
             \x20 double? get height => 2;\n\
             \x20 String get fit => 'cover';\n\
             \x20 String get repeat => 'noRepeat';\n\
             }",
};

/// The two-import collision shape every `collided_*` fixture mounts.
fn collided(source: &str) -> FileReport {
    report_ops_imports(source, &[UI_IMPORT, DECLS_IMPORT])
}

/// The corpus shape (#4294's mandate): `is!` guard + early return, then a
/// typed literal whose ELEMENT CHECKS read the members the guard's
/// continuation makes legal.
#[test]
fn collided_negative_guard_promotes_literal_elements() {
    let report = collided(
        "class Prop {\n\
         \x20 Prop(this.name, this.value);\n\
         \x20 final String name;\n\
         \x20 final String value;\n\
         }\n\
         \n\
         List<Prop> extract(Widget widget) {\n\
         \x20 if (widget is! Image) {\n\
         \x20   return const <Prop>[];\n\
         \x20 }\n\
         \x20 return <Prop>[\n\
         \x20   if (widget.width != null) Prop('w', 'wide'),\n\
         \x20   if (widget.height != null) Prop('h', 'tall'),\n\
         \x20   Prop('fit', widget.fit),\n\
         \x20   Prop('repeat', widget.repeat),\n\
         \x20 ];\n\
         }\n",
    );
    assert_eq!(texts(&report, "undefined_getter"), Vec::<String>::new());
}

/// The positive-guard control: the same collided head inside the guarded
/// branch (both arms live — no early exit involved).
#[test]
fn collided_positive_guard_promotes_inside_the_branch() {
    let report = collided(
        "String inside(Widget widget) {\n\
         \x20 if (widget is Image) {\n\
         \x20   return widget.fit + widget.repeat;\n\
         \x20 }\n\
         \x20 return 'other';\n\
         }\n",
    );
    assert_eq!(texts(&report, "undefined_getter"), Vec::<String>::new());
}

/// Every exit kind a negative guard can take: `return`, `throw`, `break`,
/// `continue` — each leaves the CONTINUING branch promoted.
#[test]
fn guard_exit_kinds_promote_oracle_clean_shapes() {
    let report = report_ops(
        "class Widget {}\n\
         \n\
         class Image extends Widget {\n\
         \x20 double? get width => 1;\n\
         \x20 String get fit => 'cover';\n\
         }\n\
         \n\
         class Other extends Widget {}\n\
         \n\
         String byReturn(Widget widget) {\n\
         \x20 if (widget is! Image) {\n\
         \x20   return 'none';\n\
         \x20 }\n\
         \x20 return widget.fit;\n\
         }\n\
         \n\
         String byThrow(Widget widget) {\n\
         \x20 if (widget is! Image) {\n\
         \x20   throw Exception();\n\
         \x20 }\n\
         \x20 return widget.fit;\n\
         }\n\
         \n\
         String byContinue(List<Widget> widgets) {\n\
         \x20 var out = '';\n\
         \x20 for (final widget in widgets) {\n\
         \x20   if (widget is! Image) {\n\
         \x20     continue;\n\
         \x20   }\n\
         \x20   out = out + widget.fit;\n\
         \x20 }\n\
         \x20 return out;\n\
         }\n\
         \n\
         String byBreak(List<Widget> widgets) {\n\
         \x20 var out = '';\n\
         \x20 for (final widget in widgets) {\n\
         \x20   if (widget is! Image) {\n\
         \x20     break;\n\
         \x20   }\n\
         \x20   out = out + widget.fit;\n\
         \x20 }\n\
         \x20 return out;\n\
         }\n",
    );
    assert_eq!(texts(&report, "undefined_getter"), Vec::<String>::new());
}

/// Local-variable receivers and nullable declared types: the guard's
/// continuation reads the promoted head in both.
#[test]
fn guard_locals_and_nullable_receivers_promote() {
    let report = report_ops(
        "class Widget {}\n\
         \n\
         class Image extends Widget {\n\
         \x20 double? get width => 1;\n\
         \x20 String get fit => 'cover';\n\
         }\n\
         \n\
         String localReceiver(Widget input) {\n\
         \x20 final Widget widget = input;\n\
         \x20 if (widget is! Image) {\n\
         \x20   return 'none';\n\
         \x20 }\n\
         \x20 return widget.fit;\n\
         }\n\
         \n\
         String nullableReceiver(Image? image) {\n\
         \x20 if (image is! Image) {\n\
         \x20   return 'none';\n\
         \x20 }\n\
         \x20 return image.fit;\n\
         }\n\
         \n\
         String nullableWidget(Widget? widget) {\n\
         \x20 if (widget is! Image) {\n\
         \x20   return 'none';\n\
         \x20 }\n\
         \x20 return widget.fit;\n\
         }\n",
    );
    assert_eq!(texts(&report, "undefined_getter"), Vec::<String>::new());
}

/// The ambient guard-free read keeps its row: the fix is collision-scoped,
/// never a blanket suppression.
#[test]
fn unguarded_member_read_still_reports() {
    let report = collided(
        "String unguarded(Widget widget) {\n\
         \x20 return widget.fit;\n\
         }\n",
    );
    assert_eq!(
        texts(&report, "undefined_getter"),
        vec!["The getter 'fit' isn't defined for the type 'Widget'.".to_owned()]
    );
}

/// A name with ONE reachable declaration keeps the resolved strict gate:/// `ui.dart`'s `Image` is unrelated to `Widget`, the oracle promotes
/// nothing, and the read stays undefined. (The ambiguity arm is about
/// attribution, not about relaxing promotion over a decidable pair.)
#[test]
fn single_declaration_name_keeps_the_resolved_gate() {
    let report = report_ops_imports(
        "String unrelated(Widget widget) {\n\
         \x20 if (widget is! Image) {\n\
         \x20   return 'none';\n\
         \x20 }\n\
         \x20 return widget.fit;\n\
         }\n",
        &[
            UI_IMPORT,
            RigImport {
                uri: "package:t/decls.dart",
                prefix: None,
                source: "class Widget {}",
            },
        ],
    );
    assert_eq!(
        texts(&report, "undefined_getter"),
        vec!["The getter 'fit' isn't defined for the type 'Widget'.".to_owned()]
    );
}

/// The legacy (pre-sound-flow) fold keeps the promotion too: a 2.19
/// fixture takes `dartq_flow`'s non-sound step, whose subtype gate cannot
/// decide a head no declaration owns — the optimistic arm covers it
/// (#4294's corpus fold ran at the workspace root's 2.x language version,
/// which is where the six false positives were emitted).
#[test]
fn legacy_language_version_keeps_the_guard_promotion() {
    let report = report_ops_bare(
        "// @dart=2.19\n\
         class Widget {}\n\
         \n\
         class Image extends Widget {\n\
         \x20 String get fit => 'cover';\n\
         }\n\
         \n\
         String byReturn(Widget widget) {\n\
         \x20 if (widget is! Image) {\n\
         \x20   return 'none';\n\
         \x20 }\n\
         \x20 return widget.fit;\n\
         }\n",
    );
    assert_eq!(texts(&report, "undefined_getter"), Vec::<String>::new());
}
