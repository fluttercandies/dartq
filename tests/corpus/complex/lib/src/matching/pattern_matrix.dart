/// The full pattern grammar in one dispatch function: constant, variable,
/// wildcard, object (incl. getter shorthand and nested relational), record,
/// list, map, relational, cast, null-check, null-assert, logical-or/and,
/// parenthesized patterns and when guards.
library;

import '../core/money.dart';
import '../core/timestamps.dart';

/// Result tags for [classify]; the closed set keeps the switch arms honest.
enum ShapeKind { number, text, money, moment, pair, listing, mapping, other }

/// Classifies an arbitrary payload with a maximal-pattern switch. Every
/// binding is referenced (guards count), relationals only run against
/// narrowed types, and no arm is fully covered by its predecessors.
ShapeKind classify(Object? payload) {
  return switch (payload) {
    // Constant patterns.
    null => ShapeKind.other,
    0 => ShapeKind.number,
    'reset' => ShapeKind.text,
    // Logical-or of constants; neither side binds a variable.
    'credit' || 'debit' => ShapeKind.text,
    // Logical-and: binding on the left, relational on the right; the bound
    // value is used in the guard.
    int amount && > 0 when amount.isOdd => ShapeKind.number,
    int() => ShapeKind.number,
    // String variable pattern; used in the guard.
    String label when label.isEmpty => ShapeKind.text,
    String() => ShapeKind.text,
    // Object pattern with a nested relational on a member. For extension
    // types the reachability checker treats the object pattern as always
    // matching, so a bare `Money()` arm after this one is flagged
    // unreachable; the relational arm is the one kept.
    Money(cents: > 0) => ShapeKind.money,
    // Object pattern with a sub-binding used in the guard.
    Instant(secondsSinceEpoch: final seconds) when seconds.isEven =>
      ShapeKind.moment,
    Instant() => ShapeKind.moment,
    // Record pattern: positional + named; all bindings used in the guard.
    (String head, int count, :final String tag)
        when count <= tag.length && head.isNotEmpty => ShapeKind.pair,
    (String head, int count) when head.length + count > 0 => ShapeKind.pair,
    // List patterns: first bound, rest collected; bindings used in guards.
    [final only] when only != null => ShapeKind.listing,
    [final head, ...var rest] when rest.isNotEmpty && head != null =>
      ShapeKind.listing,
    [_] => ShapeKind.listing,
    [] => ShapeKind.listing,
    // Map pattern with a nested cast; the cast result feeds the guard.
    {'action': final raw as String} when raw.isNotEmpty => ShapeKind.mapping,
    // Variable pattern over the remaining maps; used in the guard.
    final Map<String, Object?> loose when loose.isNotEmpty =>
      ShapeKind.mapping,
    // Parenthesized object pattern over the boxed numeric leftovers.
    (double()) => ShapeKind.other,
    _ => ShapeKind.other,
  };
}

/// Parses a restock instruction from an untyped config payload; demonstrates
/// the null-assert pattern (typed, so the guard's relational operator has a
/// real receiver), the null-check pattern and guard clauses on list and map
/// patterns.
String readRestockInstruction(Object? config) {
  return switch (config) {
    // Null-assert pattern over a typed binding: binds a non-null int or the
    // arm is skipped.
    {'units': final int units!} when units > 0 => 'restock $units',
    {'units': _} => 'restock rejected',
    {'sku': final sku, 'lane': final String lane!} => 'lane $lane for $sku',
    // Null-check pattern: matches when the value is a non-null int and
    // binds the non-null view.
    {'leadTimeDays': final int checked?} => 'lead time $checked',
    [final first, ...] => 'positional instruction starting with $first',
    final String single => 'bare instruction: $single',
    _ => 'unrecognized configuration',
  };
}

/// Null-check pattern in isolation: matches when the value is non-null and
/// binds the non-null view (contrast with the null-assert arms above,
/// which throw on null).
int? safeUnits(Object? config) => switch (config) {
      {'units': final int checked?} => checked,
      _ => null,
    };
