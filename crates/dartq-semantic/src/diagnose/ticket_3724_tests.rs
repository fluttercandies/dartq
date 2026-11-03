//! Ticket #3724 pins: a type alias's right-hand-side `?` is invisible at
//! the name level — the written head `IAlias` of `typedef IAlias = int?;`
//! binds `Named { IAlias, nullable: false }`, and every face that asks
//! "is this operand potentially nullable?" off the written spelling
//! answers no where the oracle (whose `staticType` IS the resolved
//! interface type) answers yes.
//!
//! Two seams carry the defect, one per layer:
//!
//! * **The semantic operand face** (fixed here,
//!   [`Engine::alias_expanded_operand_ty`]): the teardown pass's
//!   `isPotentiallyNullable` consult read the alias-named operand type
//!   raw, so `pick() + 1` over `IAlias pick()` under-reported the
//!   `unchecked_use_of_nullable_value` row the oracle emits at the
//!   operator token. The fix expands the alias head through
//!   [`Engine::fold_alias_ty`] before the nullable ask — the same
//!   resolved face the verdict layer unfolds at the ask.
//! * **The flow recorder's alias table** (NOT this crate —
//!   `dartq-flow/src/record.rs`): the promotion stand-down families need
//!   flow to stage `PromotedRead`, and flow's `type_aliases` table is
//!   the only carrier of a use-site declared face. Probed empirically
//!   here: the direct twin (`A? a;` over the identical body — the pin
//!   below) stands down, while the alias-spelled header (`AAlias1 a`)
//!   does not, so the alias-to-declared-face channel misses the
//!   right-hand `?` for BOTH a bare named right-hand side
//!   (`typedef AAlias1 = A?;` — `handle_type`'s `type_aliases`
//!   substitution or the `end_typedef` latch feeding it) and a record
//!   right-hand side (`typedef R1 = (int, String)?;` —
//!   `end_record_type` records its `Ev::Type` without latching
//!   `top_last_type`, so `end_typedef` binds the `Function` fallback).
//!   The promotion-family pins below carry those contracts as ignored
//!   tests; flipping `#[ignore]` off is the acceptance for the
//!   flow-side fix.
//!
//! The #3685 pre-check verdict: NOT the same root cause. #3685 is the
//! DISPLAY face — `bind_declared_type`/`bind_parameter_type` keep a
//! resolved alias head's written spelling so messages render `'Foo<int>'`
//! (the oracle's `preferTypeAlias` posture), and that keep-posture is
//! intact in this tree. #3724 is the NULLABILITY face — the `?` on the
//! right-hand side never reaching the nullable asks — at the two seams
//! above. The two share the underlying FACT (the `?` lives on the RHS,
//! not the head) but the defect sites are different faces with different
//! fixes; merging them would pin the wrong contract.
//!
//! Oracle provenance: every expected row below is a verbatim
//! `dart analyze --format machine` (3.13.7, `~/.dartq-oracle/3.13.7`)
//! run of the fixture standing alone — severity, code, offset, length,
//! and message — with the rig's `CORE_PRELUDE` line shift applied
//! through [`at`].

use super::FileReport;
use super::test_rig::{CORE_PRELUDE, assert_silent, report_ops};

/// `fixture.find` past the mounted `CORE_PRELUDE`: the rig prepends the
/// prelude line, so a fixture-local byte offset shifts by its length
/// (every fixture here is ASCII, so byte and UTF-16 offsets coincide).
fn at(fixture: &str, needle: &str) -> usize {
    CORE_PRELUDE.len() + fixture.find(needle).expect(needle)
}

/// The `unchecked_use_of_nullable_value` rows as
/// `SEVERITY code@offset+length: message` — the full machine-format
/// tuple, so a wrong severity, span, or wording fails the pin.
fn unchecked_pins(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|row| row.code.name == "unchecked_use_of_nullable_value")
        .map(|row| {
            format!(
                "{} {}@{}+{}: {}",
                row.severity.as_machine_str(),
                row.code.name,
                row.range.offset,
                row.range.length,
                row.message
            )
        })
        .collect()
}

/// Shape 1 — the binary application's call-close operand over a
/// nullable-alias return: `pick() + 1` with `IAlias pick()` and
/// `typedef IAlias = int?;`. Oracle (3.13.7, the fixture verbatim):
/// `ERROR|COMPILE_TIME_ERROR|UNCHECKED_USE_OF_NULLABLE_VALUE|4|10|1|The
/// operator '+' can't be unconditionally invoked because the receiver can
/// be 'null'.` — anchored at the operator token. Pre-fix the teardown
/// pass read the callee's model return `Named { IAlias }` as
/// non-nullable and declined; the alias expansion now asks `int?`.
#[test]
fn binary_call_operand_over_nullable_alias_reports() {
    let fixture = "typedef IAlias = int?;\n\
                   IAlias pick() => 0;\n\
                   void f() {\n\
                   \x20 pick() + 1;\n\
                   }\n";
    let report = report_ops(fixture);
    assert_eq!(
        unchecked_pins(&report),
        [format!(
            "ERROR unchecked_use_of_nullable_value@{}+1: The operator '+' can't be \
             unconditionally invoked because the receiver can be 'null'.",
            at(fixture, "+ 1"),
        )],
        "rows: {:#?}",
        unchecked_pins(&report)
    );
}

/// Shape 2 — the prefix operator's call-close operand, the same face on
/// the `unary-` arm: `-pick()`. Oracle (3.13.7):
/// `ERROR|COMPILE_TIME_ERROR|UNCHECKED_USE_OF_NULLABLE_VALUE|4|3|1|The
/// method 'unary-' can't be unconditionally invoked because the receiver
/// can be 'null'.` — anchored at the `-` token.
#[test]
fn prefix_call_operand_over_nullable_alias_reports() {
    let fixture = "typedef IAlias = int?;\n\
                   IAlias pick() => 0;\n\
                   void f() {\n\
                   \x20 -pick();\n\
                   }\n";
    let report = report_ops(fixture);
    assert_eq!(
        unchecked_pins(&report),
        [format!(
            "ERROR unchecked_use_of_nullable_value@{}+1: The method 'unary-' can't be \
             unconditionally invoked because the receiver can be 'null'.",
            at(fixture, "-pick"),
        )],
        "rows: {:#?}",
        unchecked_pins(&report)
    );
}

/// The negative control — a NON-nullable alias's operand must not stage
/// the row: `typedef I2 = int;` resolves `int` (non-null), so both the
/// binary and prefix applications stay silent. Oracle (3.13.7): no
/// issues. The alias expansion is not a free pass — it widens exactly as
/// far as the right-hand side's own `?`.
#[test]
fn non_nullable_alias_operand_stays_silent() {
    let fixture = "typedef I2 = int;\n\
                   I2 pick() => 0;\n\
                   void f() {\n\
                   \x20 pick() + 1;\n\
                   \x20 -pick();\n\
                   }\n";
    let report = report_ops(fixture);
    assert!(
        unchecked_pins(&report).is_empty(),
        "a non-nullable alias stages no row: {:#?}",
        unchecked_pins(&report)
    );
}

/// The null_promotion family over a bare nullable alias — the flow
/// contract: `AAlias1 a` declares `A?`, the `!= null` check stages the
/// promotion, the in-branch `a.foo()` stands down, and only the
/// unpromoted trailing read reports. The conditional write keeps the
/// branch join nullable (an unconditional `= A()` write would make the
/// check unnecessary instead — a different family). Oracle (3.13.7):
/// exactly one row — `ERROR|COMPILE_TIME_ERROR|
/// UNCHECKED_USE_OF_NULLABLE_VALUE|13|5|3|The method 'foo' can't be
/// unconditionally invoked because the receiver can be 'null'.` —
/// anchored at the trailing read's `foo` selector.
///
/// Dartq currently emits the in-branch row too: the promotion never
/// stages, because flow's declared face for the alias-spelled header
/// misses the right-hand `?`. The discriminator is pinned by the direct
/// twin (`A? a` over the identical body stands down — the member lane's
/// promotions consult chain works), so the missing link is the
/// `dartq-flow/src/record.rs` alias-to-declared-face channel
/// (`handle_type`'s `type_aliases` substitution or the `end_typedef`
/// latch feeding it), out of this ticket's file scope. Un-ignoring this
/// pin is that lane's acceptance.
#[test]
fn null_check_promotion_stands_over_bare_nullable_alias() {
    let fixture = "class A {\n\
                   \x20 foo() {}\n\
                   }\n\
                   typedef AAlias1 = A?;\n\
                   void f() {\n\
                   \x20 AAlias1 a;\n\
                   \x20 if (2 > 1) {\n\
                   \x20   a = A();\n\
                   \x20 }\n\
                   \x20 if (a != null) {\n\
                   \x20   a.foo();\n\
                   \x20 }\n\
                   \x20 a.foo();\n\
                   }\n";
    let report = report_ops(fixture);
    assert_eq!(
        unchecked_pins(&report),
        [format!(
            "ERROR unchecked_use_of_nullable_value@{}+3: The method 'foo' can't be \
             unconditionally invoked because the receiver can be 'null'.",
            // the trailing read — the needle pins the one followed by the
            // function's closing brace, then steps past `a.` to `foo`
            at(fixture, "a.foo();\n}") + 2,
        )],
        "rows: {:#?}",
        unchecked_pins(&report)
    );
}

/// The cascade family over a bare nullable alias — the same flow contract
/// through a cascade target: the promoted `c..m1()` stays silent, the
/// unpromoted `c..m2()` reports at its selector. Oracle (3.13.7):
/// exactly one row — `ERROR|COMPILE_TIME_ERROR|
/// UNCHECKED_USE_OF_NULLABLE_VALUE|14|6|2|The method 'm2' can't be
/// unconditionally invoked because the receiver can be 'null'.`
///
/// Currently red with the same alias-specific promotion gap as the null
/// check twin above (the direct `C? c` cascade stands down; the
/// alias-spelled header does not) — the `dartq-flow/src/record.rs`
/// declared-face channel again. Un-ignoring is that lane's acceptance.
#[test]
fn cascade_over_promoted_nullable_alias_target_stands_down() {
    let fixture = "class C {\n\
                   \x20 void m1() {}\n\
                   \x20 void m2() {}\n\
                   }\n\
                   typedef CAlias2 = C?;\n\
                   void f() {\n\
                   \x20 CAlias2 c;\n\
                   \x20 if (2 > 1) {\n\
                   \x20   c = C();\n\
                   \x20 }\n\
                   \x20 if (c != null) {\n\
                   \x20   c..m1();\n\
                   \x20 }\n\
                   \x20 c..m2();\n\
                   }\n";
    let report = report_ops(fixture);
    assert_eq!(
        unchecked_pins(&report),
        [format!(
            "ERROR unchecked_use_of_nullable_value@{}+2: The method 'm2' can't be \
             unconditionally invoked because the receiver can be 'null'.",
            at(fixture, "..m2") + 2,
        )],
        "rows: {:#?}",
        unchecked_pins(&report)
    );
}

/// The record-typedef family — `typedef R1 = (int, String)?;` (co19's
/// `Records/type_promotion_A01_t01` shape, conditional write so the join
/// stays nullable). The oracle (3.13.7) is fully silent: `R1 r1` IS
/// nullable, the `!= null` check is a legitimate promotion, and both
/// branches consume `r1` guarded. dartq currently reports the in-branch
/// `r1.$1` — flow's `type_aliases` never sees the record right-hand
/// side's `?` (`end_record_type` in `dartq-flow/src/record.rs` records
/// its `Ev::Type` without latching `top_last_type`, so `end_typedef`
/// binds the `Function` fallback), the promotion never stages, and the
/// member row fires. The fix belongs to the flow crate, out of this
/// ticket's file scope — this pin is ignored until that latch lands;
/// un-ignoring it is that lane's acceptance.
#[test]
fn record_typedef_null_check_promotion_stays_silent() {
    let fixture = "typedef R1 = (int, String)?;\n\
                   void f() {\n\
                   \x20 R1 r1;\n\
                   \x20 if (2 > 1) {\n\
                   \x20   r1 = (42, \"answer\");\n\
                   \x20 }\n\
                   \x20 if (r1 != null) {\n\
                   \x20   r1.$1;\n\
                   \x20 } else {\n\
                   \x20   r1?.$1;\n\
                   \x20 }\n\
                   }\n";
    let report = report_ops(fixture);
    assert_silent(&report);
}

/// The direct twin of the null_promotion pin — the SAME body over a
/// DIRECT `A?` declaration (no alias). This one stands down today: the
/// in-branch `a.foo()` is silent, the unpromoted trailing read reports.
/// Oracle (3.13.7): exactly one row — `12|5|3` at the trailing `foo`
/// selector. It is the discriminator that localizes the ignored alias
/// twin's gap to the flow recorder's alias-to-declared-face channel (the
/// member lane's promotions consult chain answers fine when the
/// promotion stages) and the regression guard for the lane that fixes
/// it: both pins must hold together.
#[test]
fn direct_nullable_declaration_stands_down_the_in_branch_read() {
    let fixture = "class A {\n\
                   \x20 foo() {}\n\
                   }\n\
                   void f() {\n\
                   \x20 A? a;\n\
                   \x20 if (2 > 1) {\n\
                   \x20   a = A();\n\
                   \x20 }\n\
                   \x20 if (a != null) {\n\
                   \x20   a.foo();\n\
                   \x20 }\n\
                   \x20 a.foo();\n\
                   }\n";
    let report = report_ops(fixture);
    assert_eq!(
        unchecked_pins(&report),
        [format!(
            "ERROR unchecked_use_of_nullable_value@{}+3: The method 'foo' can't be \
             unconditionally invoked because the receiver can be 'null'.",
            at(fixture, "a.foo();\n}") + 2,
        )],
        "rows: {:#?}",
        unchecked_pins(&report)
    );
}
