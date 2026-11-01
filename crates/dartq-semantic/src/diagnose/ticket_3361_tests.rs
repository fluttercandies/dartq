//! The #3361 pins — the written `void` type argument of a collection
//! literal's `<…>` group: the keyword fires no type-use row (the parser
//! routes it through `handle_void_keyword`, never `handle_type`), so the
//! list/set/map literal faces read the group from their own bytes and the
//! element is the canonical `Ty::Void` the `use_of_void_result` family
//! reads. The list spelling is pinned by #3349's un-ignored for-in arm;
//! these cover the set spelling's fire and the map value slot's silence.
//!
//! Registered through `diagnose/tests.rs`'s `#[path]` block; the module
//! stays inside the `diagnose` subtree so `crate::diagnose::test_rig`
//! resolves.

use crate::diagnose::test_rig::{CORE_PRELUDE, report_ops};
use crate::diagnose::types::FileReport;

/// The `use_of_void_result` rows of `report` as `(offset, length)`, in the
/// fixture's own coordinates, ascending.
fn void_spans(report: &FileReport) -> Vec<(u32, u32)> {
    let shift = u32::try_from(CORE_PRELUDE.len()).expect("prelude fits u32");
    let mut spans: Vec<(u32, u32)> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "use_of_void_result")
        .map(|d| (d.range.offset - shift, d.range.length))
        .collect();
    spans.sort_unstable();
    spans
}

/// The `(offset, length)` of the LAST `needle` in `fixture`.
fn span_of(fixture: &str, needle: &str) -> (u32, u32) {
    let at = u32::try_from(fixture.rfind(needle).expect("needle")).expect("offset fits u32");
    (at, u32::try_from(needle.len()).expect("len fits u32"))
}

/// The single `void` row of `fixture`, asserting there is exactly one.
fn sole_void(fixture: &str) -> (u32, u32) {
    let spans = void_spans(&report_ops(fixture));
    assert_eq!(spans.len(), 1, "expected one void row: {spans:?}");
    spans[0]
}

/// The set literal's spelling: `<void>{}` iterated into a non-`void` loop
/// variable consumes the element's value — the segment scan answers the
/// `void` argument the same way the list face does, and the for-in
/// emitter reports at the iterable.
#[test]
fn ticket_3361_for_in_void_set_literal_group_element() {
    let fixture = "void f() {\n  for (Object? o in <void>{}) {}\n}";
    assert_eq!(sole_void(fixture), span_of(fixture, "<void>{}"));
}

/// The map's `void` VALUE slot discards the entry value (the oracle's
/// `mapValueType` gate, literal_element_verifier.dart:166): the written
/// group is now a visible `Map<String, void>` row and the entry check
/// still stays silent.
#[test]
fn ticket_3361_map_void_value_slot_stays_silent() {
    let fixture = "void v() {}\nvoid f() {\n  var m = <String, void>{'a': v()};\n}";
    assert!(
        void_spans(&report_ops(fixture)).is_empty(),
        "silent: the value slot is void"
    );
}
