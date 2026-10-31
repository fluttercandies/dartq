//! #2463 — regression pins for the small census families A9–A12, A15.
//!
//! Each pin mirrors a minimal repro of the corpus site the census ⑨ audit
//! (`docs/audits/census/non-analyzer-clusters-2026-10-21.md`, families A9–A15)
//! recorded, reduced until the oracle (Dart 3.13.5 `dart analyze`) is silent
//! on it. The expectations therefore come from the oracle, never from the
//! shape of our own output.
//!
//! * A9 — a member call on a *pattern-promoted* receiver (`CustomSqlType()`
//!   inside a switch expression) inside a method that declares a same-named
//!   two-parameter method: the receiver's own `read(Object?)` wins; no
//!   `not_enough_positional_arguments` on the call.
//! * A10 — `source.sortRange(start, end, compare)` on a list of an *unbounded*
//!   element type selects `ListExtensions` (the only applicable extension);
//!   the bounded `ListComparableExtensions` alternative must not create a
//!   tie, and an unqualified call in an extension body resolves through the
//!   library scope (the top-level `shuffle`), never through the on-type's
//!   members.
//! * A11 — a chained `.map(...)` after an explicit receiver whose spelling
//!   matches a local variable (`map.entries.map(...)`) resolves on the
//!   receiver, not on the variable.
//! * A12 — the three const shapes: a generic super invocation forwarding its
//!   own type variable, a const factory redirect used inside a const
//!   initializer, and a private top-level const in a const initializer.
//! * A15 — `required super.name` associates with the resolved super
//!   constructor's named formal even when the superclass comes from an
//!   imported library.
//! * A14 (the rig-visible slice) — a concrete member a mixin application
//!   contributes satisfies an `implements` clause's abstract requirement
//!   (`EmptyUnmodifiableSet`'s layout). The SDK-model shapes of the family
//!   (`StreamView.listen`, `Stream.map`, `Isolate.kill`) need the loaded SDK
//!   and are not reachable from this no-SDK rig.
//!
//! Upstream citations: analyzer `error_verifier.dart` (`visitSuperFormalParameter`
//! :2238-2251), `extension_manager.dart` (applicability + most-specific),
//! `type_inference_engine.dart` member lookup precedence (a member selector's
//! name is never a lexical reference), and `constant.dart` (the const
//! evaluation fault set).

use super::*;
use crate::diagnose::test_rig::{
    RigImport, assert_silent, report_ops, report_ops_bare, report_ops_with, rows,
};

/// A9: the receiver's own member answers even when the enclosing class
/// declares a same-named method with a different arity, and the pattern
/// promotes the receiver to the tested interface.
#[test]
fn promoted_receiver_member_wins_over_enclosing_same_name() {
    let report = report_ops_bare(
        "abstract class BaseSqlType<T> {\n\
         \x20 T read(Object? value);\n\
         }\n\
         class CustomSqlType<T> extends BaseSqlType<T> {\n\
         \x20 @override\n\
         \x20 T read(Object? value) => throw 0;\n\
         }\n\
         class DriftAny {\n\
         \x20 final Object rawSqlValue;\n\
         \x20 const DriftAny(this.rawSqlValue) : assert(rawSqlValue is! DriftAny);\n\
         }\n\
         class ValueMapping {\n\
         \x20 T? read<T extends Object>(BaseSqlType<T> type, Object? sqlValue) {\n\
         \x20   if (sqlValue == null) return null;\n\
         \x20   return switch (type) {\n\
         \x20         CustomSqlType<T>() => type.read(sqlValue),\n\
         \x20         _ => null,\n\
         \x20       }\n\
         \x20       as T?;\n\
         \x20 }\n\
         \x20 Object wrap(Object? sqlValue) {\n\
         \x20   if (sqlValue == null) return 0;\n\
         \x20   return DriftAny(sqlValue);\n\
         \x20 }\n\
         }\n",
    );
    assert_silent(&report);
}

/// A10: two applicable extensions, the bounded one inapplicable for an
/// unbounded element type — the single applicable extension answers.
#[test]
fn unbounded_element_type_selects_the_only_applicable_extension() {
    let report = report_ops(
        "class Comparable<T> {}\n\
         extension ListExtensions<E> on List<E> {\n\
         \x20 void sortRange(int start, int end, int Function(E a, E b) compare) {}\n\
         }\n\
         extension ListComparableExtensions<E extends Comparable<E>> on List<E> {\n\
         \x20 void sortRange(int start, int end, [int Function(E a, E b)? compare]) {}\n\
         }\n\
         class ListSlice<E> {\n\
         \x20 ListSlice(this.source);\n\
         \x20 final List<E> source;\n\
         \x20 void sortRange(int start, int end, int Function(E a, E b) compare) {\n\
         \x20   source.sortRange(start, end, compare);\n\
         \x20 }\n\
         }\n",
    );
    assert_silent(&report);
}

/// A10 (second shape): an unqualified invocation inside an extension body
/// resolves in the library scope — the on-type's member of the same spelling
/// never shadows it.
#[test]
fn unqualified_call_in_extension_body_uses_library_scope() {
    let report = report_ops(
        "class Base {\n\
         \x20 void shuffle(int only) {}\n\
         }\n\
         void shuffle(Base b, int start, int end) {}\n\
         extension Ext on Base {\n\
         \x20 void shuffleRange(int start, int end) {\n\
         \x20   shuffle(this, start, end);\n\
         \x20 }\n\
         }\n",
    );
    assert_silent(&report);
}

/// A11: the chained `.map(...)` after `map.entries` resolves against the
/// receiver facet; the local variable named `map` is not consulted.
#[test]
fn chained_map_resolves_on_receiver_not_the_shadowing_variable() {
    let report = report_ops(
        "class Pair<K, V> {\n\
         \x20 String get key => '';\n\
         \x20 String get value => '';\n\
         }\n\
         class Mapped {\n\
         \x20 String join(String sep) => '';\n\
         }\n\
         class Entries {\n\
         \x20 Mapped map(String Function(Pair<String, String>) f) => Mapped();\n\
         }\n\
         class Box {\n\
         \x20 Entries get entries => Entries();\n\
         }\n\
         String query(Box map) => map.entries.map((e) => e.key).join('&');\n",
    );
    assert_silent(&report);
}

/// A12a: a const constructor forwarding its own type variable to a generic
/// super's named parameters.
#[test]
fn const_super_invocation_forwards_type_variable() {
    let report = report_ops_bare(
        "class Chg<S> {\n\
         \x20 const Chg({required this.a, required this.b});\n\
         \x20 final S a;\n\
         \x20 final S b;\n\
         }\n\
         class Trans<S> extends Chg<S> {\n\
         \x20 const Trans({required S a, required S b}) : super(a: a, b: b);\n\
         }\n",
    );
    assert_silent(&report);
}

/// A12b: a const factory redirect (`Stream.empty()`) as the argument of a
/// const constructor is a constant.
#[test]
fn const_factory_redirect_argument_is_constant() {
    let report = report_ops(
        "class Stream<T> {\n\
         \x20 const Stream.empty();\n\
         }\n\
         class ByteStream {\n\
         \x20 const ByteStream(this.stream);\n\
         \x20 final Stream<int> stream;\n\
         }\n\
         class BaseRequest {\n\
         \x20 static const ByteStream emptyBody = ByteStream(Stream<int>.empty());\n\
         }\n",
    );
    assert_silent(&report);
}

/// A12c: a private top-level const in a const constructor's initializer.
#[test]
fn private_top_level_const_in_const_initializer() {
    let report = report_ops(
        "const xsNode = _NodeType('node');\n\
         class _NodeType {\n\
         \x20 const _NodeType(this.name);\n\
         \x20 final String name;\n\
         }\n\
         class NodeTestType {\n\
         \x20 const NodeTestType([String name = 'node-test'])\n\
         \x20   : name = name,\n\
         \x20     parent = xsNode;\n\
         \x20 final String name;\n\
         \x20 final _NodeType parent;\n\
         }\n",
    );
    assert_silent(&report);
}

/// A15: `required super.certificate` associates with the named formal of a
/// super constructor declared in an imported library.
#[test]
fn super_formal_associates_across_an_imported_library() {
    // No core prelude: the mounted import directive must stay the first
    // directive of the analyzed unit (a prelude ahead of it would be the
    // `directive_after_declaration` shape instead).
    let report = report_ops_with(
        "import 'package:iface/iface.dart';\n\
         class AndroidSslAuthError extends PlatformSslAuthError {\n\
         \x20 AndroidSslAuthError({required super.certificate});\n\
         }\n",
        &[RigImport {
            uri: "package:iface/iface.dart",
            prefix: None,
            source: "class PlatformSslAuthError {\n\
                     \x20 PlatformSslAuthError({required this.certificate});\n\
                     \x20 final Object? certificate;\n\
                     }\n",
        }],
        None,
    );
    let found = rows(&report);
    assert!(
        found.is_empty(),
        "the imported super constructor's named formal associates: {found:#?}"
    );
}

/// A14 (the rig-visible shape): a concrete member the mixin application
/// contributes satisfies an `implements` clause's abstract requirement — the
/// `EmptyUnmodifiableSet extends IterableBase with UnmodifiableSetMixin
/// implements UnmodifiableSetView` layout (upstream Dart SDK source,
/// `error_verifier.dart` `_checkForAllRequiredMembers`,
/// `non_abstract_class_inherits_abstract_member`; the member lookup runs over
/// the whole interface set, mixin applications included).
#[test]
fn mixin_provided_member_satisfies_an_implements_requirement() {
    let report = report_ops(
        "abstract class Iface<E> {\n\
         \x20 void addAll(Iterable<E> elements);\n\
         }\n\
         mixin M<E> {\n\
         \x20 void addAll(Iterable<E> elements) {}\n\
         }\n\
         class Impl<E> with M<E> implements Iface<E> {}\n",
    );
    assert_silent(&report);
}

/// The same face with the mixin's `on` supertype in between (the corpus
/// layout: `extends IterableBase<E> with UnmodifiableSetMixin<E>`).
#[test]
fn on_type_mixin_member_satisfies_an_implements_requirement() {
    let report = report_ops(
        "abstract class Iface<E> {\n\
         \x20 void addAll(Iterable<E> elements);\n\
         }\n\
         abstract class Base<E> {}\n\
         mixin M<E> on Base<E> {\n\
         \x20 void addAll(Iterable<E> elements) {}\n\
         }\n\
         class Impl<E> extends Base<E> with M<E> implements Iface<E> {}\n",
    );
    assert_silent(&report);
}
