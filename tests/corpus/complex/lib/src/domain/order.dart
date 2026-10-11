/// Order aggregate built on records: order lines and totals are record
/// types, destructured with the full pattern grammar downstream.
library;

import '../core/identifiers.dart';
import '../core/money.dart';
import 'customer.dart';
import 'order_status.dart';

/// One billable line. All-named record with extension-type fields.
typedef OrderLine = ({SkuId sku, Quantity quantity, Money unitPrice});

/// Aggregated money outcome of an order: two positional fields plus the
/// named grand total.
typedef Totals = (Money subtotal, Money saved, {Money grandTotal});

/// A single pricing note attached to an order; positional record keeps the
/// trailing-comma rule exercised.
typedef PricingNote = (int ruleId, String detail);

/// The sales-order aggregate.
final class Order {
  final OrderId id;
  final Customer customer;
  OrderStatus status;
  final List<OrderLine> lines;
  final List<PricingNote> notes;

  Order({
    required this.id,
    required this.customer,
    this.status = OrderStatus.draft,
    List<OrderLine>? lines,
    List<PricingNote>? notes,
  })  : lines = lines ?? [],
        notes = notes ?? [];

  /// Line total is the only place cents arithmetic is allowed to inline.
  static Money lineTotal(OrderLine line) => line.unitPrice * line.quantity;

  Money get subtotal =>
      lines.fold(Money.zero, (sum, line) => sum + lineTotal(line));

  OrderLine? lineFor(SkuId sku) {
    for (final line in lines) {
      if (line.sku == sku) {
        return line;
      }
    }
    return null;
  }

  void addLine(SkuId sku, Quantity quantity, Money unitPrice) {
    final existing = lineFor(sku);
    if (existing == null) {
      lines.add((sku: sku, quantity: quantity, unitPrice: unitPrice));
    } else {
      _replaceLine(sku, existing.quantity + quantity, unitPrice);
    }
  }

  void _replaceLine(SkuId sku, Quantity merged, Money unitPrice) {
    final index = lines.indexWhere((line) => line.sku == sku);
    if (index >= 0) {
      lines[index] = (sku: sku, quantity: merged, unitPrice: unitPrice);
    }
  }

  /// Aggregates with a positional+named record return; callers destructure
  /// with `(subtotal, saved, grandTotal: g)`.
  Totals totals({Money Function(Money base)? discount}) {
    final base = subtotal;
    final saved = discount != null ? discount(base) : Money.zero;
    final grand = base - saved;
    return (base, saved, grandTotal: grand);
  }

  /// Pattern assignment rebinds two locals in one statement; records are
  /// immutable so the swap returns a fresh pair.
  static (Quantity, Quantity) swappedQuantities(OrderLine a, OrderLine b) {
    var (left, right) = (a.quantity, b.quantity);
    (left, right) = (right, left);
    return (left, right);
  }

  @override
  String toString() =>
      'Order($id, ${status.name}, ${lines.length} lines, ${subtotal.display()})';
}
