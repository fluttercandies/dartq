/// Order-day summaries: aggregates over orders with record destructuring,
/// folding and deterministic string assembly.
library;

import '../core/identifiers.dart';
import '../core/money.dart';
import '../domain/customer.dart';
import '../domain/order.dart';

/// Per-customer spend line; the shape mirrors the CRM record import.
typedef SpendLine = ({CustomerRef customer, Money spent, int orderCount});

/// Folds orders into per-customer spend lines, sorted by descending spend.
List<SpendLine> spendByCustomer(List<Order> orders) {
  final acc = <CustomerRef, (Money, int)>{};
  for (final order in orders) {
    final (_, _, grandTotal: grand) = order.totals();
    final (previousSpend, previousCount) =
        acc[order.customer.id] ?? (Money.zero, 0);
    acc[order.customer.id] = (
      previousSpend + grand,
      previousCount + 1,
    );
  }
  final lines = [
    for (final entry in acc.entries)
      (
        customer: entry.key,
        spent: entry.value.$1,
        orderCount: entry.value.$2,
      ),
  ];
  lines.sort((a, b) => b.spent.cents.compareTo(a.spent.cents));
  return lines;
}

/// Renders the day headline; adjacent literals keep the format readable.
String dayHeadline({
  required int orderCount,
  required Money revenue,
  required int complainants,
}) {
  final mood = complainants == 0
      ? 'clean'
      : (complainants < 3 ? 'watchful' : 'stormy');
  return 'Orders: $orderCount\n'
      'Revenue: ${revenue.display()}\n'
      'Mood: $mood';
}

/// The top tier summary; tier weight comes from the customer model.
Map<CustomerTier, int> tierHistogram(List<Customer> customers) {
  final histogram = <CustomerTier, int>{
    for (final tier in CustomerTier.values) tier: 0,
  };
  for (final customer in customers) {
    histogram[customer.tier] = (histogram[customer.tier] ?? 0) + 1;
  }
  return histogram;
}
