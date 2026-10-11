/// Pick-route planning over records: named+positional record types, list
/// patterns for route splitting and record equality for dedup.
library;

import '../core/identifiers.dart';
import '../core/money.dart';
import 'bin.dart';

/// One stop on a pick route; all-named record keeps call sites readable.
typedef PickStop = ({Bin bin, SkuId sku, int count});

/// Route outcome: positional record; the duration is seconds.
typedef RoutePlan = (List<PickStop> stops, int estimatedSeconds);

/// Plans a route: sorts stops by walking distance from the dock bin and
/// splits into waves of at most [maxStops].
RoutePlan planRoute({required Bin dock, required List<PickStop> stops, int maxStops = 12}) {
  final ordered = [...stops]..sort((a, b) {
      final da = dock.walkingDistance(a.bin);
      final db = dock.walkingDistance(b.bin);
      return da != db ? da.compareTo(db) : a.sku.value.compareTo(b.sku.value);
    });
  final wave = ordered.take(maxStops).toList();
  final seconds = 90 + wave.length * 45;
  return (wave, seconds);
}

/// Splits a route into consecutive waves with list patterns; the head/tail
/// shapes drive the wave labels.
List<String> describeWaves(List<PickStop> stops) {
  final labels = <String>[];
  var remaining = stops;
  var index = 1;
  while (remaining.isNotEmpty) {
    if (remaining case [final solo]) {
      labels.add('wave $index: single stop ${solo.sku.shortForm()}');
      break;
    }
    if (remaining case [final head, ...var tail]) {
      labels.add(
          'wave $index: ${head.sku.shortForm()} + ${tail.length} more');
      remaining = tail;
      index += 1;
    } else {
      break;
    }
  }
  return labels;
}

/// Deduplicates stops by record identity: two stops are equal when all
/// fields are equal, the record structural guarantee.
List<PickStop> deduplicateStops(List<PickStop> stops) {
  final seen = <PickStop>{};
  final unique = <PickStop>[];
  for (final stop in stops) {
    if (seen.add(stop)) {
      unique.add(stop);
    }
  }
  return unique;
}

/// Cost of walking a route; the money math stays in the money type.
Money walkingCost(RoutePlan plan) =>
    Money(2 * plan.$1.length + plan.$2 ~/ 30);
