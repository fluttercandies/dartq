//! #3229 pins — the wildcard `_` binding: a `_` local-scope declaration
//! declares NOTHING when `wildcard-variables` is enabled, so every read of
//! `_` is an unresolved name.
//!
//! Upstream skips the element at every local-scope population site — the
//! `LocalScope.add` / `FormalParameterScope` / `ConstructorInitializerScope`
//! arms (upstream Dart SDK source, pkg/analyzer/lib/src/dart/element/
//! scope.dart:537-557, :106-124, :15-35) — and the name then falls through
//! the lexical chain to the standard unresolved-simple-identifier arm.
//!
//! dartq's scope population had been gated only at
//! `Engine::add_local` (the frame's resolution set), which left two doors
//! open: the frame's declared-type map (`var_types`) still took the `_`
//! key, and `resolve_bare`'s frame walk answers a `var_types` hit as a
//! resolved local BEFORE consulting the resolution set — so
//! `int _ = 1; print(_)`, `void f(int _)`, `var _ = 1`, and the catch
//! formal all resolved silently; and an `_`-named local function inserted
//! its name into the enclosing frame's set directly. The gate now covers
//! every local-scope population surface (`Engine::declares_nothing`), and
//! the context-schema ordinal counts tracked parameter rows instead of
//! `var_types` entries (`Engine::formal_ordinal`) so a wildcard formal
//! keeps its positional slot — `(_, s)` over `void Function(int, String)`
//! still pairs `s` with `String`.
//!
//! The tracked `locals` row stays for every gate: the unused-local
//! wildcard exemption and the `_`-named local function's `dead_code` face
//! read that bookkeeping, never the resolution set.
//!
//! Registered through `diagnose/tests.rs`'s `#[path]` block (`diagnose.rs`
//! is a live-lane file out of this ticket's write scope; `#[path]` keeps
//! the module inside the `diagnose` subtree so `crate::diagnose::test_rig`
//! resolves).

use crate::diagnose::test_rig::{CORE_PRELUDE, report_ops_wildcards, rows};
use crate::diagnose::types::FileReport;

/// The offsets of one code's rows, ascending.
fn offsets(report: &FileReport, code: &str) -> Vec<u32> {
    let mut at = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| d.range.offset)
        .collect::<Vec<_>>();
    at.sort_unstable();
    at
}

/// The absolute offset of the `nth` occurrence (0-based) of `needle` in the
/// fixture as the rig analyzes it: `report_ops*` prefixes [`CORE_PRELUDE`],
/// so every fixture-local offset shifts by the prelude's length.
fn at(fixture: &str, needle: &str, nth: usize) -> u32 {
    let mut from = 0;
    for _ in 0..nth {
        from += fixture[from..]
            .find(needle)
            .expect("needle appears in the fixture")
            + needle.len();
    }
    let hit = from
        + fixture[from..]
            .find(needle)
            .expect("needle appears in the fixture");
    u32::try_from(CORE_PRELUDE.len() + hit).expect("fixture offsets fit u32")
}

/// A typed formal's `_` read is an unresolved name: the declaration binds
/// no local, so the read reports `undefined_identifier` at its own offset.
#[test]
fn formal_wildcard_read_reports_undefined() {
    let fixture = "\
void f(int _) {
  var y = _;
}
";
    let report = report_ops_wildcards(fixture, true);
    assert_eq!(
        offsets(&report, "undefined_identifier"),
        vec![at(fixture, "_;", 0)],
        "the `_` formal declares nothing, so its read is undefined: {}",
        rows(&report).join("\n")
    );
}

/// A typed local's `_` read is unresolved too (the declared-type map must
/// not answer it either).
#[test]
fn declared_local_wildcard_read_reports_undefined() {
    let fixture = "\
void f() {
  int _ = 1;
  var y = _;
}
";
    let report = report_ops_wildcards(fixture, true);
    assert_eq!(
        offsets(&report, "undefined_identifier"),
        vec![at(fixture, "_;", 0)],
        "the `int _` local declares nothing: {}",
        rows(&report).join("\n")
    );
}

/// The inferred-declarator face (`var _ = 1`) binds nothing either.
#[test]
fn inferred_local_wildcard_read_reports_undefined() {
    let fixture = "\
void f() {
  var _ = 1;
  var y = _;
}
";
    let report = report_ops_wildcards(fixture, true);
    assert_eq!(
        offsets(&report, "undefined_identifier"),
        vec![at(fixture, "_;", 0)],
        "the `var _` local declares nothing: {}",
        rows(&report).join("\n")
    );
}

/// The catch formal rides the same gate: `catch (_, st)` declares no `_`,
/// and the sibling `st` keeps resolving.
#[test]
fn catch_wildcard_read_reports_undefined() {
    let fixture = "\
void f() {
  try {
  } catch (_, st) {
    var y = _;
    var z = st;
  }
}
";
    let report = report_ops_wildcards(fixture, true);
    assert_eq!(
        offsets(&report, "undefined_identifier"),
        vec![at(fixture, "_;", 0)],
        "the catch `_` declares nothing while `st` still resolves: {}",
        rows(&report).join("\n")
    );
}

/// An `_`-named local function binds no name: calling it is
/// `undefined_function` (the `dead_code` row on the declaration is a
/// different face and stays).
#[test]
fn local_function_wildcard_call_reports_undefined_function() {
    let fixture = "\
void f() {
  void _() {}
  _();
}
";
    let report = report_ops_wildcards(fixture, true);
    assert_eq!(
        offsets(&report, "undefined_function"),
        vec![at(fixture, "_();", 0)],
        "the `_` local function declares nothing: {}",
        rows(&report).join("\n")
    );
    assert!(
        offsets(&report, "undefined_identifier").is_empty(),
        "the call position reports undefined_function, not identifier: {}",
        rows(&report).join("\n")
    );
}

/// A closure literal's `_` formal declares nothing as well.
#[test]
fn closure_formal_wildcard_read_reports_undefined() {
    let fixture = "\
void f() {
  void Function(int) g = (_) {
    var y = _;
  };
  g(1);
}
";
    let report = report_ops_wildcards(fixture, true);
    assert_eq!(
        offsets(&report, "undefined_identifier"),
        vec![at(fixture, "_;", 0)],
        "the closure `_` formal declares nothing: {}",
        rows(&report).join("\n")
    );
}

/// The wildcard formal keeps its positional slot in the context schema:
/// `(_, s)` over `void Function(int, Marked)` pairs `s` with `Marked`, so
/// `s.mark` resolves. A `var_types`-length ordinal would shift `s` onto
/// the `int` slot and report `undefined_getter 'mark'`.
#[test]
fn closure_schema_ordinal_survives_the_wildcard_slot() {
    let fixture = "\
class Marked {
  int get mark => 1;
}
void g(void Function(int, Marked) f) {}
void h() {
  g((_, s) => s.mark);
}
";
    let report = report_ops_wildcards(fixture, true);
    assert!(
        offsets(&report, "undefined_getter").is_empty(),
        "`s` must pair the second schema slot (`Marked`), never the `_` \
         formal's `int`: {}",
        rows(&report).join("\n")
    );
    assert!(offsets(&report, "undefined_method").is_empty());
    assert!(offsets(&report, "undefined_identifier").is_empty());
}

/// The constructor-initializer face (`ConstructorInitializerScope`,
/// upstream Dart SDK source, pkg/analyzer/lib/src/dart/element/scope.dart
/// :15-35): a `_` formal declares nothing there either, so the initializer
/// list's `_` read is undefined while the initialized field keeps
/// resolving.
#[test]
fn constructor_initializer_wildcard_read_reports_undefined() {
    let fixture = "\
class C {
  final int f;
  C(int _) : f = _;
}
";
    let report = report_ops_wildcards(fixture, true);
    assert_eq!(
        offsets(&report, "undefined_identifier"),
        vec![at(fixture, "= _;", 0) + 2],
        "the ctor `_` formal declares nothing in the initializer list: {}",
        rows(&report).join("\n")
    );
}

/// A declared `_` formal with no read adds no row of its own: the parameter
/// is unnamed, so no unused-parameter/prefixed-name row appears either.
#[test]
fn declared_wildcard_without_read_is_silent() {
    let fixture = "\
void f(int _) {
  var y = 1;
}
";
    let report = report_ops_wildcards(fixture, true);
    assert!(
        offsets(&report, "undefined_identifier").is_empty(),
        "a declared `_` formal with no read adds no undefined row: {}",
        rows(&report).join("\n")
    );
}

/// A top-level `_` is an ordinary library-scope name — outside every frame
/// scope the gate never applies, so the read resolves.
#[test]
fn top_level_wildcard_still_resolves() {
    let fixture = "\
var _ = 1;
void f() {
  var y = _;
}
";
    let report = report_ops_wildcards(fixture, true);
    assert!(
        offsets(&report, "undefined_identifier").is_empty(),
        "a top-level `_` keeps resolving inside bodies: {}",
        rows(&report).join("\n")
    );
}

/// Non-wildcard underscore spellings (`__`, `_x`) are ordinary names and
/// keep resolving.
#[test]
fn non_wildcard_underscore_names_resolve() {
    let fixture = "\
void f() {
  var __ = 1;
  var _x = 2;
  var y = __ + _x;
}
";
    let report = report_ops_wildcards(fixture, true);
    assert!(
        offsets(&report, "undefined_identifier").is_empty(),
        "`__` and `_x` are ordinary names: {}",
        rows(&report).join("\n")
    );
}

/// The pre-feature posture (`// @dart = 3.4`-shaped library) keeps the old
/// semantics: a lone `_` is an ordinary name and its read resolves.
#[test]
fn pre_feature_wildcard_read_resolves() {
    let fixture = "\
void f(int _) {
  var y = _;
}
";
    let report = report_ops_wildcards(fixture, false);
    assert!(
        offsets(&report, "undefined_identifier").is_empty(),
        "without the feature `_` is an ordinary name: {}",
        rows(&report).join("\n")
    );
}
