//! The #2925 pins: the instantiate-to-bounds display face adopted at the
//! freed bind/display sites — [`Engine::instantiated_head_ty`] fills a
//! bare `Named{head, EMPTY}` with its parameter defaults (`G` over
//! `typedef G<X extends A<X>> = X Function()` reads `G<A<dynamic>>`).
//!
//! 1. `bind_declared_type` (scopes_bodies.rs): a bare parameterized head
//!    in a declared-variable annotation binds the I2B face — `G source`
//!    stores `G<A<dynamic>>`, so the `G<A<Never>>` target reports the
//!    oracle's mismatch instead of staying silent on the raw heads.
//! 2. `bind_parameter_type` (scopes_bodies.rs): a formal's spelled
//!    annotation binds the same face — `testme(G source)`.
//! 3. The `G?` nullable head binds `G<A<dynamic>>?` — the use-site `?`
//!    rides the instantiation.
//! 4. `ty_display` (assignability.rs) is the message-side belt: a raw
//!    `Named{_, EMPTY}` reaching a message through a channel no bind
//!    site owns renders its instantiated face — the oracle's
//!    `DartType.getDisplayString` always reads
//!    `instantiateTypeFormalsToBounds` for a bare head (upstream Dart
//!    SDK source, `type.dart`). Display-only: the verdict `Ty` keeps
//!    its own face.
//! 5. A non-generic head (`B b = ...` over `class B {}`) stays bare —
//!    `itb_default_args` finds no registered parameters and the helper
//!    returns the written shape, so `B` never invents a group.
//!
//! The fourth adoption site — `fold_alias_ty` (resolution.rs:56) — is a
//! recorded dependency on the sibling lane owning that file; a bare
//! alias head arriving there instantiates the same way when it lands.
//!
//! Oracle provenance: the co19 `Instantiate-to-bound` typedef family
//! (`typedef_l1_t01`/`t05`/`t06`, `class_typedef_*`,
//! `nonfunction_typedef_*`, `typedef_FutureOr_l1_t01` —
//! `testme(G source)` + `F<G<FutureOr<dynamic>>> target = fsource`),
//! expected.tsv machine authority; the verdicts are
//! `invalid_assignment`/`return_of_invalid_type` rows quoting the
//! instantiated display.

use super::test_rig::{CORE_PRELUDE, report_ops};
use super::*;

fn rows_named<'a>(report: &'a FileReport, code: &str) -> Vec<(u32, u32, String)> {
    let mut rows: Vec<(u32, u32, String)> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| (d.range.offset, d.range.length, d.message.to_string()))
        .collect();
    rows.sort();
    rows
}

/// Byte offset of `needle` inside the fixture source, shifted by the
/// rig's core prelude.
fn at(source: &str, needle: &str) -> u32 {
    (CORE_PRELUDE.len() + source.find(needle).expect("needle in fixture")) as u32
}

// —— shape 1: the declared-type bind fires the instantiated face ——

/// `G source` binds `G<A<dynamic>>` (the I2B face), so assigning it to
/// `G<A<Never>>` reports the oracle's mismatch — before the adoption the
/// raw heads (`G` vs `G<A<Never>>`) asked an arity-mismatched covariant
/// question that stayed silent.
#[test]
fn declared_bare_alias_binds_the_i2b_face() {
    let source = r#"
class A<X> {}
typedef G<X extends A<X>> = X Function();
main() {
  G source = () => throw 0;
  G<A<Never>> target = source;
}
"#;
    let report = report_ops(source);
    assert_eq!(
        rows_named(&report, "invalid_assignment"),
        [(
            at(source, "source;"),
            6,
            "A value of type 'G<A<dynamic>>' can't be assigned to a variable of type 'G<A<Never>>'."
                .to_string()
        )],
        "{:?}",
        report.diagnostics
    );
}

/// The matching silent row: `G source` into a `G` target binds the same
/// `G<A<dynamic>>` on both sides — identical instantiations stay silent
/// (the fill never invents a mismatch).
#[test]
fn declared_bare_alias_into_same_head_stays_silent() {
    let source = r#"
class A<X> {}
typedef G<X extends A<X>> = X Function();
main() {
  G source = () => throw 0;
  G target = source;
}
"#;
    let report = report_ops(source);
    assert!(
        rows_named(&report, "invalid_assignment").is_empty(),
        "{:?}",
        report.diagnostics
    );
}

// —— shape 2: the formal-parameter bind ——

/// `testme(G source)` binds the formal's written `G` to `G<A<dynamic>>`
/// (the co19 `typedef_FutureOr_l1_t01` shape): the parameter's own
/// annotation is the declared type the oracle instantiates.
#[test]
fn parameter_bare_alias_binds_the_i2b_face() {
    let source = r#"
class A<X> {}
typedef G<X extends A<X>> = X Function();
testme(G source) {
  G<A<Never>> target = source;
}
"#;
    let report = report_ops(source);
    assert_eq!(
        rows_named(&report, "invalid_assignment"),
        [(
            at(source, "source;"),
            6,
            "A value of type 'G<A<dynamic>>' can't be assigned to a variable of type 'G<A<Never>>'."
                .to_string()
        )],
        "{:?}",
        report.diagnostics
    );
}

// —— shape 3: the nullable head ——

/// `G? source` binds `G<A<dynamic>>?`: the use-site `?` rides the
/// instantiation (the fold carries it through, the fill preserves it).
/// The row anchors at the READ `source` (the initializer), never the
/// declaration (oracle 3.13.7: line 6, column 25).
#[test]
fn nullable_bare_alias_binds_the_nullable_i2b_face() {
    let source = r#"
class A<X> {}
typedef G<X extends A<X>> = X Function();
main() {
  G? source;
  G<A<Never>>? target = source;
}
"#;
    let report = report_ops(source);
    assert_eq!(
        rows_named(&report, "invalid_assignment"),
        [(
            at(source, "source;\n}"),
            6,
            "A value of type 'G<A<dynamic>>?' can't be assigned to a variable of type 'G<A<Never>>?'."
                .to_string()
        )],
        "{:?}",
        report.diagnostics
    );
}

// —— shape 4: the display catch-all ——

/// `ty_display`'s belt: a raw `Named{_, EMPTY}` reaching a message
/// through a non-bind channel renders the instantiated face. A `G`
/// return annotation's stored `Ty` keeps its own face (the verdict arm
/// fills through `itb_default_args` separately), but the rendered text
/// is the oracle's `G<A<dynamic>>`.
#[test]
fn return_annotation_displays_the_i2b_face() {
    let source = r#"
class A<X> {}
typedef G<X extends A<X>> = X Function();
G f() => 0;
"#;
    let report = report_ops(source);
    let rows = rows_named(&report, "return_of_invalid_type");
    assert_eq!(rows.len(), 1, "one return_of_invalid_type row: {rows:?}");
    assert!(
        rows[0].2.contains("'G<A<dynamic>>'"),
        "the instantiated display face: {rows:?}"
    );
}

// —— shape 5: non-I2B heads stay bare ——

/// `B b` over a non-generic `class B {}`: `itb_default_args` answers
/// `None` (no registered parameters), the helper returns the written
/// `B`, and the message never invents a `B<...>` group.
#[test]
fn non_generic_head_stays_bare() {
    let source = r#"
class B {}
main() {
  B b = B();
  String s = b;
}
"#;
    let report = report_ops(source);
    assert_eq!(
        rows_named(&report, "invalid_assignment"),
        [(
            at(source, "b;"),
            1,
            "A value of type 'B' can't be assigned to a variable of type 'String'.".to_string()
        )],
        "{:?}",
        report.diagnostics
    );
}

/// The two-parameter face: `typedef H<X extends A<X>, Y extends X> = X
/// Function()` binds `H<A<dynamic>, A<X>>`-shaped defaults — the fixpoint
/// answer, not a per-parameter echo.
#[test]
fn two_param_alias_binds_the_fixpoint_defaults() {
    let source = r#"
class A<X> {}
typedef H<X extends A<X>, Y extends X> = X Function();
main() {
  H source = () => throw 0;
  H<A<Never>, A<Never>> target = source;
}
"#;
    let report = report_ops(source);
    let rows = rows_named(&report, "invalid_assignment");
    assert_eq!(rows.len(), 1, "one invalid_assignment row: {rows:?}");
    assert!(
        rows[0].2.starts_with("A value of type 'H<"),
        "the instantiated display face: {rows:?}"
    );
    assert!(
        rows[0]
            .2
            .contains("can't be assigned to a variable of type 'H<A<Never>, A<Never>>'"),
        "the written target face: {rows:?}"
    );
}
