//! #3503 pins — the null-aware element verdict reads the operand's
//! tear-off face.
//!
//! # The defect
//!
//! A `?e` collection element whose VALUE is a tear-off — a constructor
//! selector (`?C.new`, `?C.named`), a bare top-level function (`?g`), a
//! method tear-off on a created receiver (`?C().m`) — answered
//! [`Ty::UNKNOWN`]: `untyped_operand_ty` resolved only the declared
//! accessor types (getter/field), so `visitNullAwareElement`'s
//! `node.value2.staticType`-equivalent stayed silent where the oracle
//! reports the dead `?`.
//!
//! # The contract
//!
//! `handle_null_aware_element` (upstream Dart SDK source,
//! pkg/analyzer/lib/src/dart/ast/ast.dart `visitNullAwareElement` →
//! `_checkForUnnecessaryNullAware`) reports at the `?` token exactly when
//! the element value's static type is strictly non-nullable. A tear-off's
//! static type is its function face (`ConstructorReference` /
//! `PropertyAccess` / `Identifier` / `FunctionExpression`
//! `staticType`) — always non-nullable, so the `?` is dead. Send results
//! (`?g()`, `?1 + 1`), literals (`?1`, `?[1]`, `?{1: 1}`, `?(1, 2)`), and
//! `Type` literals (`?C`) are the other strictly-non-nullable spellings;
//! a declared-nullable or `null` operand keeps the operator live.
//!
//! Every expectation below was oracle-verified against
//! `~/.dartq-oracle/3.13.7/dart-sdk/bin/dart analyze --format machine`
//! (/tmp/dq3507/na.dart: `invalid_null_aware_operator` at 9:13, 9:21,
//! 9:31, 9:39, 10:13, 10:21, 10:27, 11:13, 11:22, 12:14, 12:20 and the
//! seven literal rows of line 13 — all len 1; no row beside `?maybe` /
//! `?null`) and the shared verdict's message template
//! ("The element can't be null, so the null-aware operator '?' is
//! unnecessary.", correction "Try removing the operator '?'.").

use super::test_rig::{CORE_PRELUDE, report_ops, report_ops_bare};

/// The null-aware element rows of `report` as `(code, offset, text at the
/// row)` in emission order. `source` is the prelude-prefixed fixture the
/// offsets index into ([`CORE_PRELUDE`] ahead, [`report_ops_bare`] — the
/// #3343 convention).
fn na_rows(source: &str) -> Vec<(&'static str, u32, String)> {
    let report = report_ops_bare(source);
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "invalid_null_aware_operator")
        .map(|d| {
            (
                d.code.name,
                d.range.offset,
                source[d.range.offset as usize..d.range.end() as usize].to_string(),
            )
        })
        .collect()
}

/// Offset of `needle` inside the first occurrence of `line`.
fn at(source: &str, line: &str, needle: &str) -> u32 {
    let line_at = source
        .find(line)
        .unwrap_or_else(|| panic!("fixture has {line:?}"));
    let needle_at = line
        .find(needle)
        .unwrap_or_else(|| panic!("{line:?} has {needle:?}"));
    (line_at + needle_at) as u32
}

const HEAD: &str = "class C {\n  C();\n  C.named();\n  static int f() => 0;\n  int m() => 0;\n}\nint g() => 0;\nvoid main() {\n";

/// The tear-off spellings report: the constructor selectors through the
/// declared-constructor table, the bare top-level function through its
/// signature, the block-body closure through its written parameter group,
/// and the method tear-off on a creation result through the member walk.
///
/// Oracle na.dart line 9 (`var l1 = [?C.new, ?C.named, ?g, ?() {}];`):
/// `invalid_null_aware_operator` 9:13, 9:21, 9:31, 9:39 — len 1 each —
/// and line 10's `?C().m` at 10:13.
#[test]
fn tear_off_elements_report_the_dead_operator() {
    let l1 = "  var l1 = [?C.new, ?C.named, ?g, ?() {}];\n";
    let l2 = "  var l2 = [?C().m, ?g(), ?1 + 1];\n";
    let tail = "  print([l1, l2]);\n}\n";
    let source = format!("{CORE_PRELUDE}{HEAD}{l1}{l2}{tail}");
    let q = |line: &str, needle: &str| {
        (
            "invalid_null_aware_operator",
            at(&source, line, needle),
            "?".to_string(),
        )
    };
    assert_eq!(
        na_rows(&source),
        [
            q(l1, "?C.new"),
            q(l1, "?C.named"),
            q(l1, "?g,"),
            q(l1, "?() {}"),
            q(l2, "?C().m"),
            // The send results (the send's own result face is
            // non-nullable) — already green, pinned as regression.
            q(l2, "?g(),"),
            q(l2, "?1 + 1"),
        ],
        "{:?}",
        na_rows(&source)
    );
}

/// The set-element and map-key null-aware faces share the verdict: a
/// tear-off key reports like a tear-off element.
///
/// Oracle na.dart lines 11-12 (`var s1 = {?C.new, ?() {}};
/// var m1 = {?1: 2, ?C.new: 3};`): 11:13, 11:22, 12:14, 12:20 — len 1.
#[test]
fn set_and_map_key_null_aware_faces_share_the_verdict() {
    let s1 = "  var s1 = {?C.new, ?() {}};\n";
    let m1 = "  var m1 = {?1: 2, ?C.new: 3};\n";
    let tail = "  print([s1, m1]);\n}\n";
    let source = format!("{CORE_PRELUDE}{HEAD}{s1}{m1}{tail}");
    let q = |line: &str, needle: &str| {
        (
            "invalid_null_aware_operator",
            at(&source, line, needle),
            "?".to_string(),
        )
    };
    assert_eq!(
        na_rows(&source),
        [
            q(s1, "?C.new"),
            q(s1, "?() {}"),
            q(m1, "?1"),
            q(m1, "?C.new")
        ],
        "{:?}",
        na_rows(&source)
    );
}

/// The other strictly-non-nullable spellings report; a declared-nullable
/// or `null` operand keeps the operator live.
///
/// Oracle na.dart line 13 (`var l3 = [?1, ?'a', ?true, ?C, ?[1], ?{1: 1},
/// ?(1, 2)];`): seven rows, 13:13 through 13:46, len 1 each; f-shape
/// `[?maybe, ?null]` (declared `int? maybe`): no row.
#[test]
fn literal_and_type_literal_elements_report_and_nullable_operands_stay_silent() {
    let l3 = "  var l3 = [?1, ?'a', ?true, ?C, ?[1], ?{1: 1}, ?(1, 2)];\n";
    let tail = "  print([l3]);\n}\n";
    let source = format!("{CORE_PRELUDE}{HEAD}{l3}{tail}");
    let q = |line: &str, needle: &str| {
        (
            "invalid_null_aware_operator",
            at(&source, line, needle),
            "?".to_string(),
        )
    };
    assert_eq!(
        na_rows(&source),
        [
            q(l3, "?1,"),
            q(l3, "?'a'"),
            q(l3, "?true"),
            q(l3, "?C,"),
            q(l3, "?[1]"),
            q(l3, "?{1: 1}"),
            q(l3, "?(1, 2)"),
        ],
        "{:?}",
        na_rows(&source)
    );
    let live = "void f(int? maybe) {\n  var l = [?maybe, ?null];\n  print(l);\n}\n";
    assert!(
        na_rows(&format!("{CORE_PRELUDE}{live}")).is_empty(),
        "{:?}",
        na_rows(&format!("{CORE_PRELUDE}{live}"))
    );
}

/// The shared verdict's verbatim message pair (the `_element` uniqueName's
/// template, never the replace spelling).
///
/// Oracle na.dart line 9's row text.
#[test]
fn the_verdict_keeps_the_element_message_pair() {
    let report = report_ops(&format!("{HEAD}  var l1 = [?C.new];\n  print(l1);\n}}\n"));
    let d = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "invalid_null_aware_operator")
        .expect("the tear-off element reports");
    assert_eq!(
        d.message,
        "The element can't be null, so the null-aware operator '?' is unnecessary."
    );
}
