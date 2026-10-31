//! The #2506 pins: the `.nonNulls` getter on an `Iterable<T?>` receiver.
//!
//! The corpus rows (`analyzer/lib/src/dart/element/element.dart`, the
//! `_fragments.expand(...).map((e) => e.exportedLibrary).nonNulls` chain)
//! report `undefined_getter` where the oracle reports nothing. This file
//! pins the two faces this crate owns:
//!
//! * the model registers the member — `CORE_TYPE_MEMBERS` carries
//!   `nonNulls` on every iterable-shaped head (`member_face.rs`), and the
//!   applicability shape the SDK face spells
//!   (`NullableIterableExtensions<T extends Object> on Iterable<T?>`)
//!   resolves for a nullable-element receiver, a bare receiver, a
//!   `dynamic`-argument receiver and a chained read alike;
//! * an absent member still reports, so the face above cannot swallow a
//!   real miss.
//!
//! The synthesized core-return receiver (`CORE_MEMBER_RETURNS`'
//! `("Iterable", "map", CoreRet::BareHead(..))` chain — the
//! `_fragments.expand(...).map(...).nonNulls` shape) is pinned below too:
//! its extension provenance is decided in
//! `row_resolution.rs::extension_application`, and the face must apply
//! there exactly as it does for a written declaration.

use super::test_rig::{CORE_PRELUDE, report_ops};

/// The fixture's own byte offset, shifted by the rig's prelude (which
/// `report_ops` splices ahead of the fixture on one line).
fn at(source: &str, needle: &str) -> u32 {
    (CORE_PRELUDE.len() + source.find(needle).expect("anchor")) as u32
}

/// The SDK face verbatim (`sdk/lib/collection/iterable.dart`, re-exported
/// by `core.dart:182`), plus a nullable-getter element type and a
/// `map`-shaped chain — the corpus's own ingredient set.
const FIXTURE: &str = "extension NullableIterableExtensions<T extends Object> on Iterable<T?> {\n\
                      \x20 Iterable<T> get nonNulls => throw 0;\n\
                      }\n\
                      class L {\n\
                      \x20 L? get parent => null;\n\
                      }\n\
                      class Bag<E> extends Iterable<E> {\n\
                      \x20 Iterator<E> get iterator => throw 0;\n\
                      \x20 Iterable<T> map<T>(T Function(E) f) => throw 0;\n\
                      \x20 Iterable<T> expand<T>(Iterable<T> Function(E) f) => throw 0;\n\
                      }\n\
                      void f(Iterable<L?> a, Iterable<L> b, Iterable c, Iterable<dynamic> d) {\n\
                      \x20 var x1 = a.nonNulls;\n\
                      \x20 var x2 = b.nonNulls;\n\
                      \x20 var x3 = c.nonNulls;\n\
                      \x20 var x4 = d.nonNulls;\n\
                      \x20 var x5 = a.nonNulls.nonNulls;\n\
                      \x20 var x6 = Bag<L>().map((e) => e.parent).nonNulls;\n\
                      \x20 var x7 = Bag<L>().expand((e) => [e.parent]).nonNulls;\n\
                      \x20 print(x1); print(x2); print(x3); print(x4);\n\
                      \x20 print(x5); print(x6); print(x7);\n\
                      }\n";

fn undefined_getters(report: &super::FileReport) -> Vec<u32> {
    let mut at = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "undefined_getter")
        .map(|d| d.range.offset)
        .collect::<Vec<_>>();
    at.sort_unstable();
    at
}

/// A nullable-element `Iterable`, its non-nullable twin, a bare receiver
/// and a `dynamic`-argument receiver all resolve `nonNulls`; the
/// `map`/`expand` chain that ends in the getter resolves too (the `on`
/// argument's `?` absorbs the receiver argument before the `T extends
/// Object` bound runs — applicable_extensions.dart's `applicableTo`).
#[test]
fn non_nulls_resolves_across_the_nullable_element_receiver_family() {
    let report = report_ops(FIXTURE);
    assert_eq!(
        undefined_getters(&report),
        Vec::<u32>::new(),
        "{:?}",
        report
            .diagnostics
            .iter()
            .map(|d| (d.code.name, d.range.offset, d.message.as_str()))
            .collect::<Vec<_>>()
    );
}

/// The negative control: a receiver whose type declares no `nonNulls` and
/// which no extension covers still reports the getter miss at the name.
#[test]
fn non_nulls_reports_when_no_face_defines_it() {
    let source = "class Plain {}\nvoid f(Plain p) {\n  p.nonNulls;\n}\n";
    let report = report_ops(source);
    assert_eq!(
        undefined_getters(&report),
        vec![at(source, "nonNulls")],
        "{:?}",
        report.diagnostics
    );
}

/// The getter is a member of the face's extended type, not of every
/// receiver: a receiver with no `Iterable` edge in its clause tree stays a
/// miss even though the face is in scope.
#[test]
fn non_nulls_face_does_not_cover_a_non_iterable_receiver() {
    let source = "class NotIterable { int get length => 0; }\n\
                  void f(NotIterable n) {\n  n.nonNulls;\n}\n";
    let report = report_ops(source);
    assert_eq!(
        undefined_getters(&report),
        vec![at(source, "nonNulls")],
        "{:?}",
        report.diagnostics
    );
}

/// The SYNTHESIZED core-return receiver: `map`/`expand` answered by the
/// SDK surrogate table (`CORE_MEMBER_RETURNS`'
/// `("Iterable", "map") → CoreRet::BareHead("Iterable")`) instead of a
/// written declaration. The chain hands the getter an `Iterable` head with
/// no declaration site of its own — the shape `element.dart`'s
/// `_fragments.expand(...).map(...).nonNulls` rides — so the face must
/// still apply. This is the one the #2506 report called out as residual;
/// the fixture above only covers the WRITTEN-`map` twin.
#[test]
fn non_nulls_resolves_on_a_synthesized_core_return_head() {
    let source = "extension NullableIterableExtensions<T extends Object> on Iterable<T?> {\n\
                  \x20 Iterable<T> get nonNulls => throw 0;\n\
                  }\n\
                  class Holder<E> extends Iterable<E> {\n\
                  \x20 Iterator<E> get iterator => throw 0;\n\
                  }\n\
                  class L { L? get parent => null; }\n\
                  void f(Holder<L?> h) {\n\
                  \x20 var y1 = h.map((e) => e).nonNulls;\n\
                  \x20 var y2 = h.expand((e) => [e]).nonNulls;\n\
                  \x20 print(y1); print(y2);\n\
                  }\n";
    let report = report_ops(source);
    assert_eq!(
        undefined_getters(&report),
        Vec::<u32>::new(),
        "{:?}",
        report
            .diagnostics
            .iter()
            .map(|d| (d.code.name, d.range.offset, d.message.as_str()))
            .collect::<Vec<_>>()
    );
}
