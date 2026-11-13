//! The #2937 listener-face pins: the index expression's argument check
//! (upstream `checkIndexExpressionIndex`,
//! error_detection_helpers.dart:260-289 — the folded index operand checks
//! against `parameters[0]` of every `[]`/`[]=` element the access's write
//! form applies, resolver.dart:1504 write face / :3380 read face), the
//! annotation's argument-assignability face (resolver.dart:1887 —
//! `visitAnnotation` runs `checkForArgumentTypesNotAssignableInList` on
//! the resolved constructor's formals), and the old-style function-typed
//! formal's callee signature (the desugared `FunctionType` on the
//! parameter element feeds the ordinary `resolveArgumentsToParameters`
//! walk, resolver.dart:2817-2820).
//!
//! Behavioral oracle (dart 3.13.7, the wave-5 census cluster): `a['0'] =
//! 0` reports ATA at `'0'` against `operator[]=`'s first formal; `a['0']
//! += 0` / `++a['0']` / `a['0']++` report against BOTH `[]` and `[]=`'s
//! first formals (`hasRead: true`); `a['0'];` reports against `[]`'s
//! first formal only. `@Ctx.fromInt('0')` over `const Ctx.fromInt(int p)`
//! pairs `const_constructor_param_type_mismatch` with ATA at `'0'`
//! (test_const#0's dual). `a(b(int p)) { b('0'); }` reports ATA at
//! `'0'` against the formal's `int` positional.

use super::*;
use crate::diagnose::test_rig::{CORE_PRELUDE, report_ops, rows};

/// The ATA rows of a report as sorted `(offset, length)` pairs.
fn ata_rows(report: &FileReport) -> Vec<(u32, u32)> {
    let mut out: Vec<(u32, u32)> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "argument_type_not_assignable")
        .map(|d| (d.range.offset, d.range.length))
        .collect();
    out.sort();
    out
}

/// The written span of `text` inside the fixture `source`, shifted by
/// the [`CORE_PRELUDE`] the rig mounts ahead of it (UTF-16 offsets equal
/// byte offsets for the ASCII fixtures below).
fn span_of(source: &str, text: &str) -> (u32, u32) {
    let offset =
        CORE_PRELUDE.len() as u32 + source.find(text).expect("fixture text present") as u32;
    (offset, text.len() as u32)
}

// —— 3a: index-expression argument check ——

/// The pure read face (`a['0'];`): `[]`'s first formal only — one ATA at
/// the index argument's own span (resolver.dart:3380,
/// `writeElement: null`).
#[test]
fn index_expression_read_argument_check() {
    let source = "class A {\n\
        \x20 int operator [](int index) => 0;\n\
        }\n\
        f(A a) {\n\
        \x20 a['0'];\n\
        }\n";
    let report = report_ops(source);
    assert_eq!(
        ata_rows(&report),
        vec![span_of(source, "'0'")],
        "the read face reports once at the index: {:#?}",
        rows(&report)
    );
}

/// The write face (`a['0'] = 0`): `[]=`'s first formal only — a `String`
/// `[]=` reports while the `int` `[]` read stays out (`hasRead: false`,
/// resolver.dart:1504).
#[test]
fn index_expression_write_argument_check() {
    let source = "class A {\n\
        \x20 int operator [](int index) => 0;\n\
        \x20 operator []=(String index, int value) {}\n\
        }\n\
        f(A a) {\n\
        \x20 a['0'] = 0;\n\
        }\n";
    let report = report_ops(source);
    // The `[]=` formal is `String`: `'0'` assigns — silent on BOTH faces.
    assert!(
        ata_rows(&report).is_empty(),
        "the matching write stays silent: {:#?}",
        rows(&report)
    );
    let source = "class A {\n\
        \x20 int operator [](int index) => 0;\n\
        \x20 operator []=(int index, int value) {}\n\
        }\n\
        f(A a) {\n\
        \x20 a['0'] = 0;\n\
        }\n";
    let report = report_ops(source);
    assert_eq!(
        ata_rows(&report),
        vec![span_of(source, "'0'")],
        "the write face reports once at the index: {:#?}",
        rows(&report)
    );
}

/// The compound faces (`a['0'] += 0`, `++a['0']`, `a['0']++`): read AND
/// write elements both apply (`hasRead: true`, each checked against its
/// own first formal — error_detection_helpers.dart:260-288 runs the read
/// and write checks), but two IDENTICAL rows collapse to one on the
/// oracle's report surface: probed 3.13.7 `dart analyze --format
/// machine`, an `int` `[]` + `int` `[]=` over a `String` index reports
/// the row exactly once per compound form (`+=` at 6:5, `++a[...]` at
/// 7:7, `a[...]++` at 8:5), while DIFFERENT formal spellings (a `bool`
/// `[]` beside a `String` `[]=`) report both rows — the dedup keys the
/// whole `(code, offset, message)` triple.
#[test]
fn index_expression_compound_checks_both_faces() {
    for expr in ["a['0'] += 0;", "++a['0'];", "a['0']++;"] {
        let source = format!(
            "class A {{\n\
            \x20 int operator [](int index) => 0;\n\
            \x20 operator []=(int index, int value) {{}}\n\
            }}\n\
            f(A a) {{\n\
            \x20 {expr}\n\
            }}\n"
        );
        let report = report_ops(&source);
        assert_eq!(
            ata_rows(&report),
            vec![span_of(&source, "'0'")],
            "compound {expr} reports the identical pair once: {:#?}",
            rows(&report)
        );
    }
}

/// The boundary face: an index matching its operator's first formal
/// stays silent on every write form.
#[test]
fn index_expression_matching_index_silent() {
    for expr in ["a[0];", "a[0] = 0;", "a[0] += 0;", "++a[0];", "a[0]++;"] {
        let source = format!(
            "class A {{\n\
            \x20 int operator [](int index) => 0;\n\
            \x20 operator []=(int index, int value) {{}}\n\
            }}\n\
            f(A a) {{\n\
            \x20 {expr}\n\
            }}\n"
        );
        let report = report_ops(&source);
        assert!(
            ata_rows(&report).is_empty(),
            "matching {expr} stays silent: {:#?}",
            rows(&report)
        );
    }
}

/// The dynamic-receiver boundary (`dynamic d; d['0'] = 0`): upstream's
/// element resolution answers nothing — silent both directions.
#[test]
fn index_expression_dynamic_receiver_silent() {
    let source = "f(dynamic d) {\n\
        \x20 d['0'] = 0;\n\
        \x20 d['0'] += 0;\n\
        \x20 d['0'];\n\
        }\n";
    let report = report_ops(source);
    assert!(
        ata_rows(&report).is_empty(),
        "a dynamic receiver resolves no operator: {:#?}",
        rows(&report)
    );
}

// —— 3b: annotation-argument callee link ——

/// The named-constructor annotation (`@Ctx.fromInt('0')` over
/// `const Ctx.fromInt(int p)`): the constructor's formals pair the
/// argument list — `const_constructor_param_type_mismatch` and ATA land
/// at `'0'` together (the test_const#0 dual).
#[test]
fn annotation_argument_named_ctor_reports() {
    let source = "class Ctx {\n\
        \x20 const Ctx.fromInt(int p);\n\
        }\n\
        @Ctx.fromInt('0')\n\
        main() {}\n";
    let report = report_ops(source);
    let arg = span_of(source, "'0'");
    assert_eq!(
        ata_rows(&report),
        vec![arg],
        "the annotation's ATA lands at the argument: {:#?}",
        rows(&report)
    );
    let const_rows: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "const_constructor_param_type_mismatch")
        .collect();
    assert_eq!(
        const_rows.len(),
        1,
        "the const pair lands once: {:#?}",
        rows(&report)
    );
    assert_eq!(
        (const_rows[0].range.offset, const_rows[0].range.length),
        arg,
        "the const pair anchors the same span: {:#?}",
        rows(&report)
    );
}

/// The unnamed-constructor annotation (`@Ctx('0')` over
/// `const Ctx(int p)`): the container's own spelling keys the formal
/// table.
#[test]
fn annotation_argument_unnamed_ctor_reports() {
    let source = "class Ctx {\n\
        \x20 const Ctx(int p);\n\
        }\n\
        @Ctx('0')\n\
        main() {}\n";
    let report = report_ops(source);
    assert_eq!(
        ata_rows(&report),
        vec![span_of(source, "'0'")],
        "the unnamed face reports at the argument: {:#?}",
        rows(&report)
    );
}

/// The boundary face: an argument matching the annotated constructor's
/// formal stays silent.
#[test]
fn annotation_argument_matching_silent() {
    let source = "class Ctx {\n\
        \x20 const Ctx.fromInt(int p);\n\
        }\n\
        @Ctx.fromInt(0)\n\
        main() {}\n";
    let report = report_ops(source);
    assert!(
        ata_rows(&report).is_empty(),
        "the matching annotation stays silent: {:#?}",
        rows(&report)
    );
}

// —— 3c: old-style function-typed callee ——

/// The old-style formal's signature reaches the invocation walk:
/// `a(b(int p)) { b('0'); }` counts `'0'` against the formal's `int`
/// positional — one ATA at the argument.
#[test]
fn old_style_function_typed_formal_callee_check() {
    let source = "a(b(int p)) {\n\
        \x20 b('0');\n\
        }\n";
    let report = report_ops(source);
    assert_eq!(
        ata_rows(&report),
        vec![span_of(source, "'0'")],
        "the formal's signature checks the call: {:#?}",
        rows(&report)
    );
}

/// The optionality face (`acceptFunOptBool(void fun([bool b]))` called
/// with a required-positional function): `void Function(bool)` is not a
/// subtype of `void Function([bool])` — the ordinary function-subtype
/// verdict on the captured signature.
#[test]
fn old_style_formal_optionality_mismatch() {
    let source = "void acceptFunOptBool(void fun([bool b])) {}\n\
        void funBool(bool b) {}\n\
        main() {\n\
        \x20 acceptFunOptBool(funBool);\n\
        }\n";
    let report = report_ops(source);
    assert_eq!(
        ata_rows(&report).len(),
        1,
        "the optionality mismatch reports once: {:#?}",
        rows(&report)
    );
}

/// The boundary face: a call matching the old-style formal's shape stays
/// silent.
#[test]
fn old_style_function_typed_formal_matching_silent() {
    let source = "a(b(int p)) {\n\
        \x20 b(0);\n\
        }\n";
    let report = report_ops(source);
    assert!(
        ata_rows(&report).is_empty(),
        "the matching call stays silent: {:#?}",
        rows(&report)
    );
}
