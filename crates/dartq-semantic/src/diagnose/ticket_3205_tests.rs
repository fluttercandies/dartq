//! #3205 — the const-alias metadata walk's byte-zero edge
//! (`ffi_const_native_annos`, the `_checkFfiNative` const-alias arm).
//!
//! The walk backs a cursor from the declaring name over `@name[(…)]`
//! groups; every iteration opens by probing the byte before the cursor
//! for the argument-group arm. When the cursor itself sits at 0 — a
//! name-first declaration whose name token starts the file, or the
//! annotations consumed all the way back to byte 0 — that probe read
//! `bytes.get(cursor - 1)` and underflowed (`attempt to subtract with
//! overflow`, the unused-family sweep's crasher). The guard mirrors the
//! sibling whitespace loops' own `cursor > 0` probes: at byte 0 nothing
//! can precede the cursor, the `)` arm must not run, and the walk finds
//! no further annotation and stops.
//!
//! The positive pin surfaced a second reachable panic on the same
//! `@Native` path: the omitted-`<T>` inference arm handed
//! `StringId::NONE` to the validity walk for a non-named return type
//! (an implicit return latches `Ty::Unknown`), and `StringTable::resolve`
//! rejects a never-issued id by design. Upstream keeps the Dart
//! signature's `dynamic` return (ffi_verifier.dart `_checkFfiNative`),
//! `_isValidFfiNativeFunctionType` says no, and the omitted-type arm
//! reports `native_function_missing_type` — the engine now routes the
//! shape through its own `Dynamic` verdict to that row.

use super::FileReport;
use super::test_rig::{report_ops_bare, rows};

/// The report's rows of one code as `(offset, length)`, ascending.
fn rows_of(report: &FileReport, code: &str) -> Vec<(u32, u32)> {
    let mut out = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| (d.range.offset, d.range.length))
        .collect::<Vec<_>>();
    out.sort_unstable();
    out
}

/// No row of the `@Native` declaration family is present.
#[track_caller]
fn assert_no_native_face(report: &FileReport) {
    assert!(
        !report
            .diagnostics
            .iter()
            .any(|d| d.code.name.starts_with("ffi_native")),
        "a @Native face fired on an annotation-less fixture; rows: {:#?}",
        rows(report)
    );
}

/// Shape A: the declaring name token is the file's first byte — the
/// first iteration probes `bytes.get(0 - 1)`.
#[test]
fn name_at_byte_zero_does_not_panic() {
    let report = report_ops_bare("f() {}");
    assert_no_native_face(&report);
}

/// Shape B: `@` is the file's first byte, so consuming the one
/// annotation leaves `cursor == 0` and the second iteration re-enters
/// the probe. `n` is no const alias, so no face may fire either.
#[test]
fn consumed_annotation_walks_to_byte_zero_without_panic() {
    let report = report_ops_bare("@n f() => 0;");
    assert_no_native_face(&report);
}

/// The positive path is unchanged: a const alias abutting the declaring
/// name still expands to its `Native` face. The oracle rows for the
/// non-external, omitted-`<T>`, implicit-return shape are the pair
/// `ffi_native_must_be_external` + `native_function_missing_type`, both
/// anchored at the declaring name (`_checkFfiNative` then
/// `_checkFfiNativeFunction`'s omitted-type arm over the kept `dynamic`
/// Dart signature).
#[test]
fn const_alias_walk_still_finds_the_alias() {
    let source = "const n = Native(isLeaf: true);\n@n\ng() => 0;";
    let report = report_ops_bare(source);
    let at = source.find("g()").expect("anchor") as u32;
    for code in [
        "ffi_native_must_be_external",
        "native_function_missing_type",
    ] {
        assert_eq!(
            rows_of(&report, code),
            vec![(at, 1)],
            "{code}; rows: {:#?}",
            rows(&report)
        );
    }
}
