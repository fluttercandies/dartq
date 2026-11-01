//! #3531 pins — the cross-pool `ArgListId` panic behind the `...?` spread
//! of a `FutureOr`-typed operand.
//!
//! # The defect
//!
//! `handle_spread_expression` (upstream Dart SDK source,
//! pkg/analyzer/lib/src/dart/error/error_verifier.dart `visitSpreadElement`
//! → `_checkForUnnecessaryNullAware`) asks
//! `Engine::strictly_non_nullable` whether the `...?` operand's static
//! type makes the operator dead. A `FutureOr`-head operand routed through
//! `FileElements::is_strictly_non_nullable`, whose `FutureOr` arm read
//! `self.type_args.get(args)` — the UNIT model's `ArgPool` — with an
//! `ArgListId` minted in the ENGINE's diagnosis pool (`boundary.pool()`).
//! The two pools share the id type but not the id universe: any slot the
//! listener minted past the unit clone's tail indexed another universe —
//! `len is 1 but the index is 1`, rc=4 on user input (the census's three
//! spread-case crashes: StaticSemantic_A10_t03 / A10_t09 / A11_t03).
//!
//! # The contract
//!
//! The model helper takes the owning pool across the boundary:
//! [`FileElements::is_strictly_non_nullable`] keeps the unit-pool face
//! (its ids index `self.type_args`), [`FileElements::is_strictly_non_nullable_in`]
//! resolves the `FutureOr` argument list through the pool the caller hands
//! it — the engine's `boundary.pool()`, the one that minted every `Ty` the
//! diagnose pass decides over. No slot crosses a pool boundary silently.
//!
//! # The repro
//!
//! ```dart
//! FutureOr x6;
//! List l6 = [...?x6];
//! ```
//!
//! Oracle (`~/.dartq-oracle/3.13.7/dart-sdk/bin/dart analyze`) answers one
//! row: `not_iterable_spread` at the `x6` operand, len 2 — a bare
//! `FutureOr` reads `FutureOr<dynamic>` (nullable → the `?` stays live, so
//! no `invalid_null_aware_operator`), and `FutureOr` is not an `Iterable`.
//! dartq panicked before this fix; after it, the `...?` verdict answers
//! the oracle's live-operator face (this lane's gate). The oracle's
//! `not_iterable_spread` row itself is the spread-iterability face — an
//! engine lane with no emit site yet (a known under-report, pinned silent
//! here so a future lane's green row turns this pin red on arrival).

use super::test_rig::{CORE_PRELUDE, report_ops};

/// Offset of `needle` inside the first occurrence of `line`.
fn at(source: &str, line: &str, needle: &str) -> u32 {
    let line_at = source.find(line).unwrap_or_else(|| panic!("fixture has {line:?}"));
    let needle_at = line
        .find(needle)
        .unwrap_or_else(|| panic!("{line:?} has {needle:?}"));
    (line_at + needle_at) as u32
}

/// The `(code, offset, len)` rows of `fixture`'s `invalid_null_aware_operator`
/// verdicts — the `...?` face these pins own.
fn na_rows(fixture: &str) -> Vec<(&'static str, u32, u32)> {
    report_ops(fixture)
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "invalid_null_aware_operator")
        .map(|d| (d.code.name, d.range.offset, d.range.length))
        .collect()
}

/// The headline repro panics no more. On the `...?` gate (this lane) the
/// operand `x6` reads `FutureOr<dynamic>` — nullable-possible, so the
/// operator stays live and no `invalid_null_aware_operator` lands: the
/// oracle's `...?` answer. (The oracle's `not_iterable_spread` row belongs
/// to the unimplemented spread-iterability face — see the header.)
#[test]
fn spread_of_bare_future_or_does_not_panic_and_keeps_the_operator_live() {
    let l1 = "FutureOr x6;\n";
    let l2 = "List l6 = [...?x6];\n";
    let source = format!("{CORE_PRELUDE}{l1}{l2}");
    // Panic-free completion is the first assertion; the `...?` gate's own
    // verdict is the second — zero null-aware rows on this operand.
    assert_eq!(
        na_rows(&format!("{l1}{l2}")),
        [],
        "no dead-operator row may land on a `FutureOr<dynamic>` operand"
    );
    // The oracle's `x6` operand extent is pinned so a future spread-
    // iterability lane's row lands exactly here (offset of `x6`, len 2).
    let _oracle_row_extent = (at(&source, l2, "x6"), 2u32);
}

/// The non-null `FutureOr<int>` operand answers the oracle's dead-operator
/// verdict: `invalid_null_aware_operator` at the whole 4-column `...?`
/// token. Riding the same `strictly_non_nullable` arm, this is the
/// regression twin that must not panic — the `FutureOr<int>` argument
/// list is the slot the unit pool never owned.
#[test]
fn spread_of_non_nullable_future_or_reports_the_dead_operator_without_panicking() {
    let l1 = "late FutureOr<int> v;\n";
    let l2 = "List l = [...?v];\n";
    let source = format!("{CORE_PRELUDE}{l1}{l2}");
    let rows = na_rows(&format!("{l1}{l2}"));
    assert_eq!(
        rows,
        [("invalid_null_aware_operator", at(&source, l2, "...?"), 4)],
        "{rows:?}"
    );
}

/// The nullable `FutureOr<int>?` operand keeps the `...?` live — the
/// `?`-suffixed head never enters the `FutureOr` argument read (the
/// `nullable: false` destructure skips the arm entirely). The oracle's
/// `...?` answer is silence, pinned as the arm's boundary control.
#[test]
fn spread_of_nullable_future_or_keeps_the_operator_live_without_panicking() {
    let l1 = "FutureOr<int>? v;\n";
    let l2 = "List l = [...?v];\n";
    assert_eq!(
        na_rows(&format!("{l1}{l2}")),
        [],
        "no dead-operator row may land on a `FutureOr<int>?` operand"
    );
}
