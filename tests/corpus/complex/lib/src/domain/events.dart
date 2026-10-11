/// Inventory event stream: a sealed event hierarchy dispatched with the
/// object-pattern grammar, plus the append-only log.
library;

import '../core/identifiers.dart';
import '../core/money.dart';
import '../core/timestamps.dart';

sealed class InventoryEvent {
  final Instant occurredAt;

  const InventoryEvent({required this.occurredAt});
}

final class StockReceived extends InventoryEvent {
  final SkuId sku;
  final Quantity amount;
  final Money unitCost;

  const StockReceived({
    required super.occurredAt,
    required this.sku,
    required this.amount,
    required this.unitCost,
  });
}

final class StockAdjusted extends InventoryEvent {
  final SkuId sku;
  final Quantity delta;
  final String reason;

  const StockAdjusted({
    required super.occurredAt,
    required this.sku,
    required this.delta,
    required this.reason,
  });
}

final class StockDepleted extends InventoryEvent {
  final SkuId sku;

  const StockDepleted({required super.occurredAt, required this.sku});
}

/// One-line audit description per event; exhaustive switch expression with
/// getter-shorthand object patterns and a guard clause.
String describeEvent(InventoryEvent event) => switch (event) {
      StockReceived(:final sku, :final amount, :final unitCost) =>
        'received ${sku.shortForm()} x${amount.units} @ ${unitCost.display()}',
      StockAdjusted(:final sku, delta: final d, :final reason) when d.units < 0 =>
        'removed ${-d.units} of ${sku.shortForm()} ($reason)',
      StockAdjusted(:final sku, :final reason) =>
        'adjusted ${sku.shortForm()} ($reason)',
      StockDepleted(:final sku) => 'depleted ${sku.shortForm()}',
    };

/// Net quantity effect of an event; zero when the event carries no amount.
Quantity quantityEffect(InventoryEvent event) => switch (event) {
      StockReceived(amount: final amount) => amount,
      StockAdjusted(delta: final delta) => delta,
      StockDepleted() => Quantity.zero,
    };

/// Append-only event log with a generic predicate scan.
class EventLog {
  final List<InventoryEvent> _events = [];

  void append(InventoryEvent event) => _events.add(event);

  int get length => _events.length;

  Iterable<InventoryEvent> eventsSince(Instant cutoff) =>
      _events.where((event) => event.occurredAt.isAfter(cutoff));

  List<InventoryEvent> whereKind(bool Function(InventoryEvent) test) =>
      [for (final event in _events) if (test(event)) event];

  /// Net stock effect for one SKU across the whole log.
  Quantity netEffectFor(SkuId sku) {
    var total = Quantity.zero;
    for (final event in _events) {
      if (event case StockReceived(sku: final s, amount: final amount)
          when s == sku) {
        total += amount;
      } else if (event case StockAdjusted(sku: final s, delta: final delta)
          when s == sku) {
        total += delta;
      }
    }
    return total;
  }
}
