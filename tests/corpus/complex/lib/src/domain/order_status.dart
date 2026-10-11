/// Order lifecycle states as an enhanced enum: carried data, derived
/// predicates, a closed transition table and a parse entry point.
library;

enum OrderStatus implements Comparable<OrderStatus> {
  draft(0),
  submitted(1),
  paid(2),
  shipped(3),
  delivered(4),
  cancelled(99);

  const OrderStatus(this.rank);

  final int rank;

  bool get isTerminal => switch (this) {
        OrderStatus.delivered || OrderStatus.cancelled => true,
        _ => false,
      };

  bool get isOpen => !isTerminal && this != OrderStatus.draft;

  static const List<OrderStatus> fulfillmentPath = [
    submitted,
    paid,
    shipped,
    delivered,
  ];

  /// Allowed direct transitions; cancelled is reachable from any open state.
  bool canTransitionTo(OrderStatus next) => switch (this) {
        OrderStatus.draft => next == submitted || next == cancelled,
        OrderStatus.submitted => next == paid || next == cancelled,
        OrderStatus.paid => next == shipped || next == cancelled,
        OrderStatus.shipped => next == delivered,
        OrderStatus.delivered || OrderStatus.cancelled => false,
      };

  static OrderStatus? tryParse(String raw) => switch (raw) {
        'draft' => draft,
        'submitted' => submitted,
        'paid' => paid,
        'shipped' => shipped,
        'delivered' => delivered,
        'cancelled' => cancelled,
        _ => null,
      };

  @override
  int compareTo(OrderStatus other) => rank.compareTo(other.rank);
}
