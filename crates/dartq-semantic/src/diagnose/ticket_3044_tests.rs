//! #3044 pins: the `unused_element_parameter` constructor-formals matrix —
//! positional / named / field-formal `this.x` / super-param `super.x`, each
//! × (used-via-call / used-in-body / unused), plus the non-call read faces
//! (`this.sel(...)` redirects, `C.sel`/`C.new` tear-offs, `@C.sel` metadata)
//! and the call-shape boundary (`new`/`const` keyword calls credit only the
//! supplied arguments). Upstream law (unused_local_elements_verifier.dart):
//! a formal reports iff no invocation binds it
//! (`_addParametersForArguments`), no non-call read credits the whole list
//! (`visitSimpleIdentifier`'s tear-off arm), and no `_isUsedElement`
//! exemption applies (required / generic-class ctor / public-accessible /
//! super-corresponding-required / overriding-used). Oracle: dart 3.13.1.

use super::test_rig::{CORE_PRELUDE, report_ops};
use crate::FileReport;

/// The `(offset, length)` pairs of `unused_element_parameter` rows,
/// source-ordered.
fn rows(report: &FileReport) -> Vec<(u32, u32)> {
    let mut rows: Vec<(u32, u32)> = report
        .diagnostics
        .iter()
        .filter(|row| row.code.name == "unused_element_parameter")
        .map(|row| (row.range.offset, row.range.length))
        .collect();
    rows.sort_unstable();
    rows.dedup();
    rows
}

/// `(offset, 1)` of the `nth` identifier-bounded `name` occurrence — every
/// neighbor byte must be a non-identifier char, so `"a"` never hits the
/// `a` inside `class` or `final`. Shifted past [`CORE_PRELUDE`]; ASCII
/// fixtures make byte and UTF-16 offsets coincide.
fn name_at(source: &str, name: &str, nth: usize) -> (u32, u32) {
    let ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let bytes = source.as_bytes();
    let found = source
        .match_indices(name)
        .filter(|(i, _)| {
            let before_ok = i.checked_sub(1).is_none_or(|p| !ident(bytes[p]));
            let after_ok = bytes.get(i + name.len()).is_none_or(|b| !ident(*b));
            before_ok && after_ok
        })
        .nth(nth)
        .unwrap_or_else(|| panic!("`{name}` occurrence #{nth} missing in {source:?}"));
    (found.0 as u32 + CORE_PRELUDE.len() as u32, 1)
}

fn check(source: &str, expected: Vec<(u32, u32)>) {
    assert_eq!(rows(&report_ops(source)), expected, "source: {source}");
}

// --- positional optional -------------------------------------------------

/// Positional `[p]`: unused reports the param span; a positional argument
/// binds it; a body read of the bare name resolves to the FIELD for a
/// `this.p` formal and does NOT spare the parameter (3.13.1: the frame's
/// locals removal is oracle-correct — `print(p)` over `this.p` reports).
#[test]
fn positional_matrix() {
    // Unused.
    check(
        "class _A {\n  _A([int? p]);\n}\n",
        vec![name_at("class _A {\n  _A([int? p]);\n}\n", "p", 0)],
    );
    // Used via call.
    check("class _A {\n  _A([int? p]);\n}\nvar a = _A(1);\n", vec![]);
    // Field-formal read in body still reports (body resolves to field).
    check(
        "class _C {\n  int a = 0;\n  _C({this.a = 0}) { print(a); }\n}\nint f() => _C().a;\n",
        vec![name_at(
            "class _C {\n  int a = 0;\n  _C({this.a = 0}) { print(a); }\n}\nint f() => _C().a;\n",
            "a",
            1,
        )],
    );
    // `new`/`const` keyword calls supply no arguments → still report.
    check(
        "class _A {\n  _A([int? a]);\n}\nvar b = new _A();\n",
        vec![name_at(
            "class _A {\n  _A([int? a]);\n}\nvar b = new _A();\n",
            "a",
            0,
        )],
    );
    check(
        "class _A {\n  const _A([int? a]);\n}\nconst b = const _A();\nvar c = b;\n",
        vec![name_at(
            "class _A {\n  const _A([int? a]);\n}\nconst b = const _A();\nvar c = b;\n",
            "a",
            0,
        )],
    );
    // Supplied argument spares.
    check(
        "class _A {\n  _A([int? a]);\n}\nvar b = new _A.new(0);\n",
        vec![],
    );
}

// --- named optional ------------------------------------------------------

/// Named `{p}`: unused reports; `C(p: e)` binds it; an unsupplied named
/// call reports.
#[test]
fn named_matrix() {
    check(
        "class _A {\n  _A({int? p});\n}\n",
        vec![name_at("class _A {\n  _A({int? p});\n}\n", "p", 0)],
    );
    check(
        "class _A {\n  _A({int? p});\n}\nvar a = _A(p: 1);\n",
        vec![],
    );
    check(
        "class _A {\n  _A({int? p});\n}\nvar a = _A();\n",
        vec![name_at(
            "class _A {\n  _A({int? p});\n}\nvar a = _A();\n",
            "p",
            0,
        )],
    );
    // A public primary-constructor name on a private class still reports
    // (the class itself is not publicly accessible).
    check(
        "class _A.named({int? a});\nf() => _A.named();\n",
        vec![name_at(
            "class _A.named({int? a});\nf() => _A.named();\n",
            "a",
            0,
        )],
    );
    check(
        "class _A._named({int? a});\nf() => _A._named();\n",
        vec![name_at(
            "class _A._named({int? a});\nf() => _A._named();\n",
            "a",
            0,
        )],
    );
}

// --- field-formal `this.x` ------------------------------------------------

/// Field-formals register on the param face: unbound `[this.f]` reports,
/// a bound one spares.
#[test]
fn field_formal_matrix() {
    check(
        "class _A {\n  final int? f;\n  _A([this.f]);\n}\n",
        vec![name_at(
            "class _A {\n  final int? f;\n  _A([this.f]);\n}\n",
            "f",
            1,
        )],
    );
    check(
        "class _A {\n  final int? f;\n  _A([this.f]);\n}\nvar a = _A(1);\n",
        vec![],
    );
    check(
        "class _A {\n  final int? f;\n  _A({this.f});\n  factory _A.named({int? f}) = _A;\n}\nf() => _A.named(f: 0);\n",
        vec![],
    );
}

// --- super-param `super.x` ------------------------------------------------

/// `super.x` is an ordinary optional formal of ITS constructor: bound
/// spares, unbound reports — except when the super-corresponding
/// parameter is required (upstream exemption).
#[test]
fn super_param_matrix() {
    check(
        "class _A {\n  final int? e;\n  _A({this.e});\n}\nclass _B extends _A {\n  _B({super.e});\n}\nvar b = _B(e: 2);\n",
        vec![],
    );
    // The super-corresponding required `b` exempts the unbound
    // `super.b` formal entirely.
    check(
        "class A {\n  A({required this.a, required this.b});\n  final String a;\n  final String b;\n}\nclass _B extends A {\n  _B({required super.a, super.b = 'b'});\n}\nvar foo = _B(a: 'a');\n",
        vec![],
    );
    // A super-initializer invocation binds the SUPER parameter.
    check(
        "class _A {\n  final int? e;\n  _A({this.e});\n}\nclass _B extends _A {\n  _B([int? e]) : super(e: 1);\n}\nvar b = _B(1);\n",
        vec![],
    );
    check(
        "class _A {\n  final int? e;\n  _A([this.e]);\n}\nclass _B extends _A {\n  _B(int e) : super(e);\n}\nvar b = _B(1);\n",
        vec![],
    );
}

// --- non-call reads --------------------------------------------------------

/// A `this.sel(...)` redirecting initializer reads the target's whole
/// formal list (tear-off arm — never per-argument); a bare `this()`
/// carries no selector and credits nothing.
#[test]
fn this_redirect_matrix() {
    // `this._t()` credits `p`; the redirecting ctor's own `q` still
    // reports.
    check(
        "class _B {\n  _B._t([int? p]);\n  _B.sel([int? q]) : this._t();\n}\nvar b = _B.sel();\n",
        vec![name_at(
            "class _B {\n  _B._t([int? p]);\n  _B.sel([int? q]) : this._t();\n}\nvar b = _B.sel();\n",
            "q",
            0,
        )],
    );
    check(
        "class _B {\n  _B._t([int? p]);\n  _B.sel([int? q]) : this._t(0);\n}\nvar b = _B.sel();\n",
        vec![name_at(
            "class _B {\n  _B._t([int? p]);\n  _B.sel([int? q]) : this._t(0);\n}\nvar b = _B.sel();\n",
            "q",
            0,
        )],
    );
    // Bare `this()`: no selector → both formals report.
    check(
        "class _A {\n  _A([int? p]);\n  _A.sel([int? q]) : this();\n}\nvar a = _A.sel();\n",
        vec![
            name_at(
                "class _A {\n  _A([int? p]);\n  _A.sel([int? q]) : this();\n}\nvar a = _A.sel();\n",
                "p",
                0,
            ),
            name_at(
                "class _A {\n  _A([int? p]);\n  _A.sel([int? q]) : this();\n}\nvar a = _A.sel();\n",
                "q",
                0,
            ),
        ],
    );
}

/// A bare `C.sel` / `C.new` / `C<T>.sel` reference is a tear-off read —
/// the whole formal list credits (upstream `visitSimpleIdentifier`'s
/// non-call arm).
#[test]
fn tearoff_matrix() {
    check(
        "class _A {\n  _A._named([int? a]);\n}\nvar f = _A._named;\n",
        vec![],
    );
    check("class _A {\n  _A([int? a]);\n}\nvar f = _A.new;\n", vec![]);
    check(
        "class _A {\n  _A.new2([int? a]);\n}\nvar f = _A.new2;\n",
        vec![],
    );
    check(
        "class _A<T> {\n  _A([int? a]);\n}\nvar f = _A<int>.new;\n",
        vec![],
    );
    // The call-shape boundary: `(` after the selector credits only the
    // supplied arguments (zero → report).
    check(
        "class _A {\n  _A([int? a]);\n}\nvar b = _A.new();\n",
        vec![name_at(
            "class _A {\n  _A([int? a]);\n}\nvar b = _A.new();\n",
            "a",
            0,
        )],
    );
    check(
        "class _A {\n  _A._named([int? a]);\n}\nvar b = new _A._named();\n",
        vec![name_at(
            "class _A {\n  _A._named([int? a]);\n}\nvar b = new _A._named();\n",
            "a",
            0,
        )],
    );
    check(
        "class _A {\n  const _A._named([int? a]);\n}\nvar b = const _A._named();\n",
        vec![name_at(
            "class _A {\n  const _A._named([int? a]);\n}\nvar b = const _A._named();\n",
            "a",
            0,
        )],
    );
}

// --- the remaining oracle matrix ------------------------------------------

/// Enums, the generic-class exemption (#47839), factory-redirect pairing
/// (`_matchParameters`), and the default-value / assert faces.
#[test]
fn misc_oracle_matrix() {
    // Enum optional formals report.
    check(
        "enum _E {\n  v;\n  const _E({int a = 0});\n}\n",
        vec![name_at(
            "enum _E {\n  v;\n  const _E({int a = 0});\n}\n",
            "a",
            0,
        )],
    );
    check(
        "enum _E {\n  v;\n  const _E([int a = 0]);\n}\n",
        vec![name_at(
            "enum _E {\n  v;\n  const _E([int a = 0]);\n}\n",
            "a",
            0,
        )],
    );
    // An enum constant's argument binds the parameter.
    check("enum _E {\n  v(0);\n  const _E([int a = 0]);\n}\n", vec![]);
    // A generic-class constructor's formals are exempt (#47839).
    check(
        "class C<T> {\n  C._([int? x]);\n}\nvoid foo() {\n  C._(7);\n}\n",
        vec![],
    );
    // Redirecting factory pairs its own formals to the target's — an
    // unpaired target formal reports, a paired one spares.
    check(
        "class _A {\n  final int? f;\n  _A({this.f});\n  factory _A.named() = _A;\n}\nf() => _A.named();\n",
        vec![name_at(
            "class _A {\n  final int? f;\n  _A({this.f});\n  factory _A.named() = _A;\n}\nf() => _A.named();\n",
            "f",
            1,
        )],
    );
    check(
        "class _A {\n  final int? f;\n  _A({this.f});\n  factory _A.named({int? f}) = _A;\n}\nf() => _A.named(f: 0);\n",
        vec![],
    );
    check(
        "class _A {\n  final int? f;\n  _A([this.f]);\n  factory _A.named([int? a]) = _A;\n}\nf() => _A.named(0);\n",
        vec![],
    );
    // A param read in an `assert` initializer counts as use.
    check(
        "class _C {\n  _C([int? a]) : assert(a != null);\n}\nint f() => _C(1).hashCode;\n",
        vec![],
    );
}
