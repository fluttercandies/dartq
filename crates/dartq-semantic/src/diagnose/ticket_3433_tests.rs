//! The #3433 pins — `dot_shorthand_undefined_member` at the
//! declaration-initializer face, plus the rider's `InvalidType`
//! catch-display pin.
//!
//! # The wrong fact
//!
//! A leading-dot shorthand's selector (`E e = .a`) never merges a
//! receiver: `push_member` writes it as a `Bare(Plain)` + `qualified`
//! row with `ty = Unknown`, and the row-resolution fold treats that
//! shape as silently resolved. A member MISS therefore reported
//! nothing — 510 oracle rows repo-wide dartq never emitted.
//!
//! # The contract
//!
//! - The declaration group's written annotation (`pending_decl_ty`)
//!   folded through `fold_alias_ty` + `future_or_base` is the ONLY
//!   context a shorthand resolves against here — upstream's
//!   `DotShorthandPropertyAccess` / `DotShorthandInvocation`
//!   `_resolveTargetInterfaceElement` shape. No armed annotation, an
//!   unresolvable one, or a non-interface shape = no decidable context.
//! - Context `Ty::Named` + a static member or constructor hit types the
//!   selector row with the member's declared face — silent. A miss
//!   reports `dot_shorthand_undefined_member` at the selector, read and
//!   invocation arms alike (upstream's `dotShorthandUndefinedGetter`
//!   and `dotShorthandUndefinedInvocation` freeze to the same code).
//! - The invocation arm reports with the written context's display when
//!   the context is not an accessible interface type, `'_'` when no
//!   context exists at all (`var c = .m()`); the read arm suppresses
//!   that family (`dot_shorthand_missing_context`, a sibling code
//!   dartq does not emit).
//! - `Intent::Write`/`ReadWrite`/`Operator` selectors (`x == .a`,
//!   `.a = v`) keep their silence: their context is the operand's, not
//!   the declaration's — a separate under-report this ticket does not
//!   claim.
//!
//! Oracle provenance: `dart analyze` 3.13.7 truth table —
//! `DotShorthandPropertyAccessResolutionTest.test_error_unresolved`,
//! `DotShorthandInvocationResolutionTest.test_error_context_none` /
//! `test_error_context_invalid`, conformance `expected.tsv:2340-2343,
//! 2394-2395, 2423` — the row anchors the SELECTOR span with the
//! context display in slot two.

use super::test_rig::{CORE_PRELUDE, RigImport, code_names, report_ops, report_ops_imports, rows};
use crate::diagnose::FileReport;

/// The absolute offset of the first `needle` in `fixture` as the rig
/// analyzes it: `report_ops` prefixes [`CORE_PRELUDE`], so every
/// fixture-local offset shifts by the prelude's length.
fn at(fixture: &str, needle: &str) -> u32 {
    let hit = fixture
        .find(needle)
        .unwrap_or_else(|| panic!("{needle:?} not in fixture:\n{fixture}"));
    u32::try_from(CORE_PRELUDE.len() + hit).expect("fixture offsets fit u32")
}

/// The `dot_shorthand_undefined_member` rows as `(offset, length)`
/// pairs, ascending — the anchors below assert the selector span.
fn shorthand_rows(report: &FileReport) -> Vec<(u32, u32)> {
    let mut at: Vec<(u32, u32)> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "dot_shorthand_undefined_member")
        .map(|d| (d.range.offset, d.range.end() - d.range.offset))
        .collect();
    at.sort_unstable();
    at
}

/// Whether `report` carries a `dot_shorthand_undefined_member` row —
/// the silent pins assert on this code alone (the fixtures' own
/// `unused_local_variable`/`invalid_assignment` noise is orthogonal to
/// the shorthand verdict and stays out of the contract).
fn has_shorthand_row(report: &FileReport) -> bool {
    code_names(report).contains(&"dot_shorthand_undefined_member")
}

// —— enum shorthand hit: `E e = .a` resolves `.a` against the context
// —— `E` (a static member of the enum's interface) — no shorthand row.
#[test]
fn enum_shorthand_hit_stays_silent() {
    let r = report_ops("enum E { a, b }\nvoid f() { E e = .a; }\n");
    assert!(
        !has_shorthand_row(&r),
        "an enum member hit resolves silently: {:?}",
        rows(&r)
    );
}

// —— enum shorthand miss: `.c` is no member of `E` — the row lands at
// —— the `c` selector, length 1 (conformance `expected.tsv:7694`'s
// —— `2:10|1` anchor shape).
#[test]
fn enum_shorthand_miss_reports_at_selector() {
    let source = "enum E { a, b }\nvoid f() { E e = .c; }\n";
    let r = report_ops(source);
    assert_eq!(
        shorthand_rows(&r),
        vec![(at(source, "c;"), 1)],
        "one miss row at the `c` selector: {:?}",
        rows(&r)
    );
}

// —— no context: `var x = .a` resolves nothing and reports nothing
// —— (the read arm's `dot_shorthand_missing_context` verdict is a
// —— sibling code dartq does not emit — the suppression is the pin).
#[test]
fn no_context_shorthand_stays_silent() {
    let r = report_ops("void f() { var x = .a; }\n");
    assert!(
        !has_shorthand_row(&r),
        "a context-free read carries no undefined-member row: {:?}",
        rows(&r)
    );
}

// —— static member hit: `C c = .x` where `C` declares `static const x`
// —— resolves `.x` against `C`'s static namespace — no shorthand row
// —— (the `invalid_assignment`/`unused_local_variable` rows the fixture
// —— otherwise carries are the orthogonality the helper filters).
#[test]
fn static_member_hit_stays_silent() {
    let r = report_ops("class C { static const int x = 0; }\nvoid f() { C c = .x; }\n");
    assert!(
        !has_shorthand_row(&r),
        "a static member hit resolves silently: {:?}",
        rows(&r)
    );
}

// —— static method hit: `C c = .m()` resolves `.m` against `C`'s static
// —— namespace — no shorthand row (the invocation face joins the member
// —— face).
#[test]
fn static_method_hit_stays_silent() {
    let r = report_ops("class C { static int m() => 0; }\nvoid f() { C c = .m(); }\n");
    assert!(
        !has_shorthand_row(&r),
        "a static method hit resolves silently: {:?}",
        rows(&r)
    );
}

// —— constructor hit: `C c = .named()` where `C.named` is declared
// —— answers `C` itself — no shorthand row.
#[test]
fn named_constructor_hit_stays_silent() {
    let r = report_ops("class C { C.named(); }\nvoid f() { C c = .named(); }\n");
    assert!(
        !has_shorthand_row(&r),
        "a named-ctor hit resolves silently: {:?}",
        rows(&r)
    );
}

// —— `.new` against a class WITH an unnamed constructor answers `C` —
// —— silent (`ctor_face_exists`'s `has_unnamed_constructor` arm).
#[test]
fn new_constructor_hit_stays_silent() {
    let r = report_ops("class C { C(); }\nvoid f() { C c = .new(); }\n");
    assert!(
        !has_shorthand_row(&r),
        "an unnamed-ctor hit resolves silently: {:?}",
        rows(&r)
    );
}

// —— `.new` against a class WITHOUT an unnamed constructor misses:
// —— `ctor_face_exists` answers false and the invocation arm reports —
// —— conformance `expected.tsv:2344`'s `test_error_unresolved_new`.
#[test]
fn new_constructor_miss_reports() {
    let source = "class C { C.named(); }\nvoid f() { C c = .new(); }\n";
    let r = report_ops(source);
    assert_eq!(
        shorthand_rows(&r),
        vec![(at(source, "new"), 3)],
        "the `.new` selector reports when no unnamed face exists: {:?}",
        rows(&r)
    );
}

// —— CRITICAL negative: an aliased context resolves through
// —— `fold_alias_ty` — `E2 e = .a` reads `.a` against `E` — no row.
#[test]
fn alias_context_shorthand_hit_stays_silent() {
    let r = report_ops("enum E { a, b }\ntypedef E2 = E;\nvoid f() { E2 e = .a; }\n");
    assert!(
        !has_shorthand_row(&r),
        "the aliased context resolves `.a` against `E`: {:?}",
        rows(&r)
    );
}

// —— the same alias fold arms the miss: `.c` is no member of `E`
// —— through `E2` either — the row lands at the selector.
#[test]
fn alias_context_shorthand_miss_reports() {
    let source = "enum E { a, b }\ntypedef E2 = E;\nvoid f() { E2 e = .c; }\n";
    let r = report_ops(source);
    assert_eq!(
        shorthand_rows(&r),
        vec![(at(source, "c;"), 1)],
        "the alias resolves to `E` and the miss still reports: {:?}",
        rows(&r)
    );
}

// —— instance member miss: `C c = .m` where `m` is INSTANCE-only —
// —— `walk_members` finds it but `is_static` vetoes, so the read arm
// —— reports (upstream `dotShorthandUndefinedGetter` —
// —— conformance `expected.tsv:2394`'s `test_error_unresolved` family).
#[test]
fn instance_member_read_reports() {
    let source = "class C { int get m => 0; }\nvoid f() { C c = .m; }\n";
    let r = report_ops(source);
    assert_eq!(
        shorthand_rows(&r),
        vec![(at(source, "m;"), 1)],
        "an instance member is no static-namespace hit: {:?}",
        rows(&r)
    );
}

// —— instance member invocation: `C c = .m()` — the invoke arm reports
// —— the same code (`dotShorthandUndefinedInvocation` freezes to
// —— `dot_shorthand_undefined_member`) — conformance
// —— `expected.tsv:2342`'s `test_error_notStatic`.
#[test]
fn instance_member_invocation_reports() {
    let source = "class C { int m() => 0; }\nvoid f() { C c = .m(); }\n";
    let r = report_ops(source);
    // `m(` matches the member declaration `int m(` first — anchor on
    // `.m` so the SHORTHAND's selector is the assertion's target.
    assert_eq!(
        shorthand_rows(&r),
        vec![(at(source, ".m(") + 1, 1)],
        "an instance method is no static-namespace hit: {:?}",
        rows(&r)
    );
}

// —— context-free invocation: `var c = .m()` reports `'_'` upstream —
// —— conformance `expected.tsv:2341`'s `test_error_context_none`. The
// —— row still lands at the selector.
#[test]
fn no_context_invocation_reports() {
    let source = "void f() { var c = .m(); }\n";
    let r = report_ops(source);
    assert_eq!(
        shorthand_rows(&r),
        vec![(at(source, "m("), 1)],
        "a context-free invocation reports `'_'`: {:?}",
        rows(&r)
    );
}

// —— function-typed context invocation: `C Function() c = .m()`
// —— displays the written head — conformance `expected.tsv:2340`'s
// —— `test_error_context_invalid`. The context is decidable as a Ty
// —— but not a `Ty::Named` head, so the invoke arm reports with the
// —— written spelling.
#[test]
fn function_context_invocation_reports_written_head() {
    let source = "class C {}\nvoid f() { C Function() c = .m(); }\n";
    let r = report_ops(source);
    assert_eq!(
        shorthand_rows(&r),
        vec![(at(source, "m("), 1)],
        "a non-interface context still reports the invocation: {:?}",
        rows(&r)
    );
    let row = r
        .diagnostics
        .iter()
        .find(|d| d.code.name == "dot_shorthand_undefined_member")
        .unwrap_or_else(|| panic!("no shorthand row: {:?}", rows(&r)));
    assert!(
        row.message.contains("C Function()"),
        "the written context displays: {:?}",
        row.message
    );
}

// —— imported-class ctor hit: `import 'lib.dart'; C c = .named()`
// —— resolves `.named` through `resolve_container` +
// —— `ctor_face_exists` — the sibling unit's face answers the union
// —— the local-only `ctor_decl_site` cannot see.
#[test]
fn imported_named_constructor_hit_stays_silent() {
    let r = report_ops_imports(
        "import 'package:t/lib.dart';\nvoid f() { C c = .named(); }\n",
        &[RigImport {
            uri: "package:t/lib.dart",
            prefix: None,
            source: "class C { C.named(); }\n",
        }],
    );
    assert!(
        !has_shorthand_row(&r),
        "the imported face's named ctor resolves: {:?}",
        rows(&r)
    );
}

// —— the imported miss still reports: `.other` is no member of the
// —— imported `C` — the union answers None and the row lands.
#[test]
fn imported_context_miss_reports() {
    let r = report_ops_imports(
        "import 'package:t/lib.dart';\nvoid f() { C c = .other(); }\n",
        &[RigImport {
            uri: "package:t/lib.dart",
            prefix: None,
            source: "class C { C.named(); }\n",
        }],
    );
    assert!(
        has_shorthand_row(&r),
        "a miss against the imported face reports: {:?}",
        rows(&r)
    );
}

// —— a non-shorthand selector stays silent: `x.foo` merges into a
// —— Member row (never `Bare`), so the gate rejects it — no
// —— `dot_shorthand_undefined_member` even on a miss of `foo`.
#[test]
fn receiver_dot_access_never_reports_shorthand() {
    let r = report_ops("class C { void foo() {} }\nvoid f(C x) { x.foo(); }\n");
    assert!(
        !has_shorthand_row(&r),
        "a receiver-dot access is not a shorthand: {:?}",
        rows(&r)
    );
}

// —— cascade sections (`x..foo`) keep their silence too: the section's
// —— selector merges through the cascade receiver — never a `Bare` row.
#[test]
fn cascade_section_never_reports_shorthand() {
    let r = report_ops("class C { void foo() {} }\nvoid f(C x) { x..foo(); }\n");
    assert!(
        !has_shorthand_row(&r),
        "a cascade section is not a shorthand: {:?}",
        rows(&r)
    );
}

// —— the operand-context shorthand keeps its documented silence:
// —— `x == .a` resolves `.a` against `x`'s type upstream (an
// —— `Intent::Operator` selector this pass does not claim — the
// —— under-report direction the ticket scopes out).
#[test]
fn equality_operand_shorthand_keeps_scoped_silence() {
    let r = report_ops("enum E { a, b }\nvoid f(E x) { x == .a; }\n");
    assert!(
        !has_shorthand_row(&r),
        "the operand context is a separate face: {:?}",
        rows(&r)
    );
}

// —— field-initializer window: `class W { E e = .a; }` — the
// —— `finish_const_initializer` hook fires for field initializers too
// —— (the declaration group's annotation arms `pending_decl_ty` the
// —— same way).
#[test]
fn field_initializer_shorthand_resolves() {
    let r = report_ops("enum E { a, b }\nclass W { E e = .a; }\n");
    assert!(
        !has_shorthand_row(&r),
        "the field window resolves `.a` silently: {:?}",
        rows(&r)
    );
}

// —— and the field-initializer MISS reports identically.
#[test]
fn field_initializer_shorthand_miss_reports() {
    let source = "enum E { a, b }\nclass W { E e = .c; }\n";
    let r = report_ops(source);
    assert_eq!(
        shorthand_rows(&r),
        vec![(at(source, "c;"), 1)],
        "the field window's miss reports at the selector: {:?}",
        rows(&r)
    );
}

// —— a two-variable group sweeps both selectors: the first miss and a
// —— hit share the `init_mark` window — the miss reports once, the hit
// —— upgrades and stays silent.
#[test]
fn grouped_declaration_sweeps_each_selector() {
    let source = "enum E { a, b }\nvoid f() { E x = .c, y = .a; }\n";
    let r = report_ops(source);
    assert_eq!(
        shorthand_rows(&r),
        vec![(at(source, "c,"), 1)],
        "the miss reports and the hit stays silent: {:?}",
        rows(&r)
    );
}

// —— the second window does not re-verdict the first: a stale
// —— `init_mark` resweep meets upgraded rows (hit) and a deduped emit
// —— (miss) — one row per selector across two declaration groups.
#[test]
fn stale_mark_resweep_stays_idempotent() {
    let source = "enum E { a, b }\nvoid f() { E x = .c; E y = .c; }\n";
    let r = report_ops(source);
    let at_rows = shorthand_rows(&r);
    assert_eq!(
        at_rows.len(),
        2,
        "each window reports once: {:?}",
        rows(&r)
    );
    assert!(
        at_rows.iter().all(|&(_, len)| len == 1),
        "both rows anchor a one-char selector: {at_rows:?}"
    );
}

// —— Rider pin (`catch_ty_display`): the malformed2_test_09 shape —
// —— `on List<String>` first, then `on Unresolved9` — deads the second
// —— clause because the unresolved head folds to `InvalidType` and
// —— `InvalidType <: List<String>` holds through subtype.dart's 0c arm.
// —— The `dead_code_on_catch_subtype` message displays the SUBSTITUTED
// —— invalid head as `'InvalidType'` — the oracle's
// —— `getDisplayString` — where `Ty::render`'s `Unknown` arm spelled
// —— `""`.
#[test]
fn rider_invalid_on_clause_renders_invalid_type() {
    let r = report_ops(
        "void f() { try { throw 0; } on List<String> {} on Unresolved9 catch (e) {} }\n",
    );
    let row = r
        .diagnostics
        .iter()
        .find(|d| d.code.name == "dead_code_on_catch_subtype")
        .unwrap_or_else(|| panic!("no dead_code_on_catch_subtype row: {:?}", rows(&r)));
    assert!(
        row.message.contains("'InvalidType' is a subtype of 'List<String>'"),
        "the invalid head spells InvalidType: {:?}",
        row.message
    );
}

// —— The malformed2_test_11 shape — `on List<Unresolved10>` first, then
// —— `on Unresolved11` — folds the FIRST clause's written head to
// —— `List<InvalidType>` (the substitution fills the argument, not the
// —— whole head), so the message's SECOND slot keeps the written
// —— `List<Unresolved10>` head while the first displays `InvalidType`.
#[test]
fn rider_invalid_argument_keeps_written_head() {
    let r = report_ops(
        "void f() { try { throw 0; } on List<Unresolved10> {} on Unresolved11 catch (e) {} }\n",
    );
    let row = r
        .diagnostics
        .iter()
        .find(|d| d.code.name == "dead_code_on_catch_subtype")
        .unwrap_or_else(|| panic!("no dead_code_on_catch_subtype row: {:?}", rows(&r)));
    assert!(
        row
            .message
            .contains("'InvalidType' is a subtype of 'List<Unresolved10>'"),
        "the written head survives the argument fill: {:?}",
        row.message
    );
}

// —— Rider regression pin: an ordinary catch-subtype message is
// —— unchanged — resolved heads render their own spelling, no
// —— `InvalidType` splice anywhere.
#[test]
fn rider_ordinary_catch_message_unchanged() {
    let r = report_ops(
        "void f() { try { throw 0; } on String {} on Object {} }\n",
    );
    assert!(
        r.diagnostics
            .iter()
            .all(|d| !d.message.contains("InvalidType")),
        "a resolved head renders its own spelling: {:?}",
        rows(&r)
    );
}
