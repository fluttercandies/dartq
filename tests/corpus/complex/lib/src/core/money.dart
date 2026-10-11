/// Monetary value type: integer cents wrapped in an extension type, with the
/// operator surface a pricing engine needs. No `implements` clause: stays on
/// the conservative 3.13 extension-type shape.
library;

extension type Money(int cents) {
  /// Shared zero; the implicit representation constructor is not const, so
  /// the canonical zero is a lazily-initialized final instead.
  static final Money zero = Money(0);

  bool get isNegative => cents < 0;
  bool get isZero => cents == 0;

  Money operator +(Money other) => Money(cents + other.cents);
  Money operator -(Money other) => Money(cents - other.cents);
  Money operator *(Quantity scale) => Money(cents * scale.units);
  Money operator -() => Money(-cents);

  bool operator <(Money other) => cents < other.cents;
  bool operator <=(Money other) => cents <= other.cents;
  bool operator >(Money other) => cents > other.cents;
  bool operator >=(Money other) => cents >= other.cents;

  Money clampTo(Money low, Money high) {
    if (this < low) {
      return low;
    }
    if (this > high) {
      return high;
    }
    return this;
  }

  /// Splits the amount into [parts] near-equal shares, remainder on the
  /// first share; parts must be positive.
  List<Money> split(int parts) {
    final base = cents ~/ parts;
    var remainder = cents % parts;
    return [
      for (var i = 0; i < parts; i++)
        Money(base + (remainder-- > 0 ? 1 : 0)),
    ];
  }

  /// Renders the amount with a decimal point; extension types cannot
  /// declare `toString` (an Object member name), so callers format
  /// explicitly through this method.
  String display() {
    final sign = cents < 0 ? '-' : '';
    final abs = cents.abs();
    final whole = abs ~/ 100;
    final frac = (abs % 100).toString().padLeft(2, '0');
    return '$sign$whole.$frac';
  }
}

/// Quantity of units priced together with money.
extension type Quantity(int units) {
  static final Quantity zero = Quantity(0);

  bool get isEmpty => units <= 0;

  Quantity operator +(Quantity other) => Quantity(units + other.units);
  Quantity operator -(Quantity other) => Quantity(units - other.units);

  Quantity clampToZero() => units < 0 ? Quantity(0) : this;
}
