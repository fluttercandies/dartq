//! #3757 pins: the #3465 verdict-dead-branch synthesis is sound-flow-gated.
//!
//! An `unnecessary_type_check` / `unnecessary_null_comparison` verdict
//! synthesizes a `dead_code` row for the killed branch only when
//! `sound_flow_analysis` is on (`// @dart >= 3.9`): the oracle's
//! `dead_code` there rides the flow engine, and a pre-3.9 unit keeps the
//! verdict row while emitting NO `dead_code`
//! (`// @dart = 3.8`, oracle dart 3.13.7 — both fixtures verified against
//! it in both modes).

use super::test_rig::{CORE_PRELUDE, report_ops, report_ops_bare};
use crate::FileReport;

/// The `(offset, length)` pairs of `code` rows, source-ordered.
fn rows_of(report: &FileReport, code: &str) -> Vec<(u32, u32)> {
    report
        .diagnostics
        .iter()
        .filter(|row| row.code.name == code)
        .map(|row| (row.range.offset, row.range.length))
        .collect()
}

/// Byte offset of `needle`'s `nth` occurrence inside `source`, shifted
/// past [`CORE_PRELUDE`]. ASCII fixtures make byte and UTF-16 offsets
/// coincide.
fn at(source: &str, needle: &str, nth: usize) -> u32 {
    source
        .match_indices(needle)
        .nth(nth)
        .unwrap_or_else(|| panic!("`{needle}` occurrence #{nth} missing in {source:?}"))
        .0 as u32
        + CORE_PRELUDE.len() as u32
}

// —— Sound flow on (default, `// @dart >= 3.9`) ————————————————————————

/// `x != null` over a non-nullable `int x`: the verdict lands AND the
/// else arm synthesizes `dead_code` (oracle: verdict at `x != null`,
/// `dead_code` at the else body).
#[test]
fn null_comparison_synthesizes_dead_else() {
    let source = "void f(int x) {\n\
                  \x20 if (x != null) {\n\
                  \x20   print(1);\n\
                  \x20 } else {\n\
                  \x20   print(2);\n\
                  \x20 }\n\
                  }\n";
    let report = report_ops(source);
    // The verdict anchors at `!=` (oracle machine `2|9|7`), the dead row
    // at the else block's `{` through its `}` (oracle `4|10|19`).
    assert_eq!(
        rows_of(&report, "unnecessary_null_comparison"),
        vec![(at(source, "!=", 0), 7)],
        "source: {source}"
    );
    let else_open = at(source, "} else {", 0) + 7;
    assert_eq!(
        rows_of(&report, "dead_code"),
        vec![(else_open, 19)],
        "source: {source}"
    );
}

/// `x is int` over a non-nullable `int x`: the verdict lands AND the
/// else arm synthesizes `dead_code` (oracle: verdict at `x is int`,
/// `dead_code` at the else body).
#[test]
fn type_check_synthesizes_dead_else() {
    let source = "void g(int x) {\n\
                  \x20 if (x is int) {\n\
                  \x20   print(1);\n\
                  \x20 } else {\n\
                  \x20   print(2);\n\
                  \x20 }\n\
                  }\n";
    let report = report_ops(source);
    // The verdict spans `x is int` (oracle machine `2|7|8`), the dead row
    // the else block's braces (oracle `4|10|19`).
    assert_eq!(
        rows_of(&report, "unnecessary_type_check"),
        vec![(at(source, "x is int", 0), 8)],
        "source: {source}"
    );
    let else_open = at(source, "} else {", 0) + 7;
    assert_eq!(
        rows_of(&report, "dead_code"),
        vec![(else_open, 19)],
        "source: {source}"
    );
}

// —— Sound flow off (`// @dart = 3.8`) ——————————————————————————————————

/// The same null comparison under `// @dart = 3.8`: the verdict row stays
/// (it is not flow-gated) while NO `dead_code` is synthesized — the
/// oracle emits the verdict alone there.
///
/// [`report_ops_bare`] — not [`report_ops`] — because the `@dart` marker
/// must sit on the fixture's first line for `fixture_language_version`
/// to read it; the prelude would leave the unit at the default 3.15
/// (sound-flow on).
#[test]
fn null_comparison_pre39_emits_no_dead_code() {
    let source = "// @dart=3.8\n\
                  void f(int x) {\n\
                  \x20 if (x != null) {\n\
                  \x20   print(1);\n\
                  \x20 } else {\n\
                  \x20   print(2);\n\
                  \x20 }\n\
                  }\n";
    let report = report_ops_bare(source);
    assert_eq!(
        rows_of(&report, "unnecessary_null_comparison").len(),
        1,
        "source: {source}"
    );
    assert_eq!(
        rows_of(&report, "dead_code"),
        Vec::<(u32, u32)>::new(),
        "source: {source}"
    );
}

/// The same type check under `// @dart = 3.8`: verdict row stays, NO
/// `dead_code` — the oracle emits the verdict alone there.
#[test]
fn type_check_pre39_emits_no_dead_code() {
    let source = "// @dart=3.8\n\
                  void g(int x) {\n\
                  \x20 if (x is int) {\n\
                  \x20   print(1);\n\
                  \x20 } else {\n\
                  \x20   print(2);\n\
                  \x20 }\n\
                  }\n";
    let report = report_ops_bare(source);
    assert_eq!(
        rows_of(&report, "unnecessary_type_check").len(),
        1,
        "source: {source}"
    );
    assert_eq!(
        rows_of(&report, "dead_code"),
        Vec::<(u32, u32)>::new(),
        "source: {source}"
    );
}
