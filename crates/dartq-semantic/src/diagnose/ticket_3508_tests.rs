//! The #3508 pins — three `finalize_usage` arms iterated the drained
//! `self.rows` and returned empty; each arm now re-derives from the
//! surviving pools (`refs` / `member_refs` / `member_reads`).
//!
//! `resolve_rows` `mem::take`s the row stack before `finalize_usage`
//! runs, so the three arms' `self.rows.iter()` probes always produced an
//! empty set — the same silent-empty failure #3477 fixed. Each face below
//! pins the verdict the arm was written to produce, so the pin fails if
//! the re-derived pool ever goes silent again.
//!
//! Registered here via `#[path]`; the module stays inside the `diagnose`
//! subtree so `crate::diagnose::test_rig` resolves.

use super::test_rig::{report_ops, rows};
use super::FileReport;

/// The `unused_element`/`unused_field`/`unused_local_variable` rows of
/// `report`, rendered for the failure message.
fn unused(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| {
            d.code.name == "unused_element"
                || d.code.name == "unused_field"
                || d.code.name == "unused_local_variable"
        })
        .map(|d| format!("{}@{}: {}", d.code.name, d.range.offset, d.message))
        .collect()
}

// —— `missing_write_setters` / `missing_write_top_levels` ————————————————
//
// A bare `_s = 1` write to a name no plain-name lookup resolves lands
// `RefTarget::Missing` at push time; `bare_write_target_exists` rescues
// the write silently, so `member_refs` carries no edge. The drained row
// stack was the arm that re-resolved the setter spelling — the `refs`
// pool's `Missing` + `pure_write_at` probe now feeds it. Without the
// re-derived arm the setter element reports `unused_element` even though
// the oracle's `usedElements.members` carries it.

/// The implicit-this setter face: `set _s` written bare inside its own
/// class stays used (upstream `preferSetter`, lexical_lookup.dart:22-35).
#[test]
fn bare_write_credits_implicit_setter() {
    let fixture = "\
class A {
  set _s(int v) {}
  void m() {
    _s = 1;
  }
}
";
    let report = report_ops(fixture);
    let bad = unused(&report);
    assert!(
        !bad.iter().any(|r| r.contains("'_s'")),
        "the bare `_s = 1` write resolves the `set _s` accessor: {bad:?}\n{}",
        rows(&report).join("\n")
    );
}

/// The top-level setter face: `set _top` written bare from a function
/// body resolves the accessor, not a variable — the setter stays used.
#[test]
fn bare_write_credits_top_level_setter() {
    let fixture = "\
set _top(int v) {}
void f() {
  _top = 1;
}
";
    let report = report_ops(fixture);
    let bad = unused(&report);
    assert!(
        !bad.iter().any(|r| r.contains("'_top'")),
        "the bare `_top = 1` write resolves the `set _top` accessor: {bad:?}\n{}",
        rows(&report).join("\n")
    );
}

// —— `write_offsets` (the `_isReadIdentifier` face) —————————————————————
//
// A write `v = 3` credits the SETTER element, never the variable — the
// variable still reports unless a read also lands. The re-derived
// `write_offsets` set (`refs` `pure_write_at` probes joined with
// `member_refs` `Written` edges) is what keeps the write from reading the
// variable's name into `top_level_used`/`ref_member_reads`.

/// A private top-level variable written but never read reports
/// `unused_element` — the `refs` `pure_write_at` arm keeps `_v = 1` out
/// of `top_level_used`.
#[test]
fn written_only_top_level_variable_reports() {
    let fixture = "\
int _v = 0;
void f() {
  _v = 1;
}
";
    let report = report_ops(fixture);
    let bad = unused(&report);
    assert!(
        bad.iter().any(|r| r.contains("'_v'")),
        "the `_v = 1` write never reads the variable: {bad:?}\n{}",
        rows(&report).join("\n")
    );
}

/// The read twin: a top-level `_w` read in expression position stays
/// silent — the same `refs` entry's non-write offset never lands in
/// `write_offsets`.
#[test]
fn read_top_level_variable_stays_silent() {
    let fixture = "\
int _w = 0;
int f() => _w;
";
    let report = report_ops(fixture);
    let bad = unused(&report);
    assert!(
        !bad.iter().any(|r| r.contains("'_w'")),
        "the `_w` return reads the variable: {bad:?}\n{}",
        rows(&report).join("\n")
    );
}

// —— `shorthand_ctor_refs` ——————————————————————————————————————————————
//
// A leading-dot constructor shorthand (`._named()`, `.new()`) credits the
// named constructor's element — `refs` filtered on
// `dot_shorthand_context` + `!byte_before_dot_is_question` + `byte_at(end)
// == '('` carries the selector spelling. Without the arm the ctor site
// reports `unused_element`.

/// `C make() => ._named();` credits `C._named`'s element — the shorthand
/// invocation is a ctor use the `member_refs` edge pool does not carry.
#[test]
fn shorthand_named_ctor_stays_silent() {
    let fixture = "\
class C {
  C._named();
}
C make() => ._named();
";
    let report = report_ops(fixture);
    let bad = unused(&report);
    assert!(
        !bad.iter().any(|r| r.contains("'_named'")),
        "the `._named()` shorthand invokes the private ctor: {bad:?}\n{}",
        rows(&report).join("\n")
    );
}
