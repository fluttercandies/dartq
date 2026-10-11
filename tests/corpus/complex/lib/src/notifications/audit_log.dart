/// Audit notifications: the async surface of the corpus — futures, async
/// generators, await-for drains and future composition.
library;

import '../core/identifiers.dart';
import '../core/money.dart';
import '../core/timestamps.dart';
import '../domain/events.dart';

/// Sentinel emitted when no deplete event arrives before the deadline.
final InventoryEvent timeoutSentinel = StockAdjusted(
  occurredAt: Instant.epoch,
  sku: SkuId('-'),
  delta: Quantity.zero,
  reason: 'no depleted event before timeout',
);

/// Append-and-forget audit sink.
class AuditLog {
  final List<InventoryEvent> _events = [];

  int get length => _events.length;

  /// Recording is asynchronous to model the remote sink; ordering is kept
  /// by awaiting the append before returning.
  Future<void> record(InventoryEvent event) async {
    _events.add(event);
    await Future<void>.delayed(Duration.zero);
  }

  /// Live event stream; async* with early skip on the cutoff.
  Stream<InventoryEvent> changes({Instant? since}) async* {
    for (final event in _events) {
      if (since != null && !event.occurredAt.isAfter(since)) {
        continue;
      }
      yield event;
    }
  }

  /// Replays events at or before [cutoff] by delegation.
  Stream<InventoryEvent> replayAtOrBefore(Instant cutoff) =>
      changes().where((event) => !event.occurredAt.isAfter(cutoff));

  /// Drains the change stream with await-for and folds a count.
  Future<int> drainDepletedCount() async {
    var seen = 0;
    await for (final event in changes()) {
      if (event is StockDepleted) {
        seen += 1;
      }
    }
    return seen;
  }

  /// Waits for the first deplete event or times out to null; the nullable
  /// widening goes through [Future.then] so the timeout handler can return
  /// null in a type-correct way.
  Future<InventoryEvent?> firstDepleted({
    Duration timeout = const Duration(seconds: 5),
  }) {
    return changes()
        .firstWhere(
          (event) => event is StockDepleted,
          orElse: () => timeoutSentinel,
        )
        .then<InventoryEvent?>((event) => event)
        .timeout(timeout, onTimeout: () => null);
  }
}
