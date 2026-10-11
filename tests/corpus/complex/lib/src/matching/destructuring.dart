/// Destructuring surfaces: switch expressions over enums and lists,
/// record destructuring in for-in loops, if-case chains and record
/// re-assignment.
library;

import '../core/identifiers.dart';

/// Pick-route severity ladder.
enum RouteFlag { clear, congested, blocked }

/// Parses a warehouse code line like `Z3-A12-S5` into its parts.
(String zone, String aisle, int shelf) parseLocationCode(String code) {
  final parts = code.split('-');
  if (parts case [final zone, final aisle, final shelfDigits]) {
    final shelf = int.tryParse(shelfDigits) ?? 0;
    return (zone, aisle, shelf);
  }
  return ('?', '?', 0);
}

/// For-in record destructuring over a list of pairs.
Map<String, int> tallyByZone(List<(String zone, int picks)> activity) {
  final tally = <String, int>{};
  for (final (zone, picks) in activity) {
    tally[zone] = (tally[zone] ?? 0) + picks;
  }
  return tally;
}

/// if-case chains with list/map patterns and relational guards.
String triage(RouteFlag flag, Map<String, Object?> context) {
  if (flag case RouteFlag.blocked) {
    return 'blocked: reroute now';
  }
  if (context case {'queueDepth': final int depth} when depth > 50) {
    return 'congested: queue depth $depth';
  }
  if (context case {'note': final String note}) {
    return 'clear: $note';
  }
  return 'clear';
}

/// Switch expression with a list pattern splitting head/tail; the four
/// length shapes cover every list, so no fallback arm is needed.
String summarizeQueue(List<String> pending) => switch (pending) {
      [] => 'queue empty',
      [final single] => 'one pending: $single',
      [final first, final second] => 'two pending: $first, $second',
      [final first, ..., final last] =>
        '${pending.length} pending: $first..$last',
    };

/// Builds SKU summaries, exercising record types as parameters and named
/// record construction in both directions.
typedef PickSummary = (SkuId sku, int picks, {bool rush});

PickSummary highestPriority(List<PickSummary> summaries) {
  var best = summaries.first;
  for (final candidate in summaries) {
    // The pattern must mirror the record type's positional/named SHAPE
    // exactly: `picks` is positional in PickSummary, only `rush` is named.
    // A shape mismatch makes the checker dispatch every field as Object?
    // (PATTERN_TYPE_MISMATCH_IN_IRREFUTABLE_CONTEXT); per-field annotations
    // do not participate, so bare variable patterns bind the field types.
    final (candidateSku, candidatePicks, rush: candidateRush) = candidate;
    final (bestSku, bestPicks, rush: bestRush) = best;
    final outranks = candidateRush && !bestRush ||
        candidateRush == bestRush &&
            (candidatePicks > bestPicks ||
                candidatePicks == bestPicks &&
                    candidateSku.value.compareTo(bestSku.value) < 0);
    if (outranks) {
      best = candidate;
    }
  }
  return best;
}

/// Record re-assignment swaps through the pattern-assignment form.
(List<String>, List<String>) rebalanceQueues(
  List<String> left,
  List<String> right,
) {
  var (a, b) = (left, right);
  (a, b) = (b, a);
  return (a, b);
}
