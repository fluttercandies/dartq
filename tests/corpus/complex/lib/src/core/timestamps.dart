/// Time primitives for the domain: a const-constructible instant and a
/// half-open window over it, both deeply used by scheduling code.
library;

/// A UTC timestamp at second granularity. Deliberately const-only: it is
/// stored in const tables and used as a default field initializer.
class Instant implements Comparable<Instant> {
  final int secondsSinceEpoch;

  const Instant(this.secondsSinceEpoch);

  static const Instant epoch = Instant(0);

  Instant operator +(Duration delta) =>
      Instant(secondsSinceEpoch + delta.inSeconds);

  Instant operator -(Duration delta) =>
      Instant(secondsSinceEpoch - delta.inSeconds);

  bool isBefore(Instant other) => secondsSinceEpoch < other.secondsSinceEpoch;
  bool isAfter(Instant other) => secondsSinceEpoch > other.secondsSinceEpoch;

  @override
  int compareTo(Instant other) =>
      secondsSinceEpoch.compareTo(other.secondsSinceEpoch);

  @override
  bool operator ==(Object other) =>
      other is Instant && other.secondsSinceEpoch == secondsSinceEpoch;

  @override
  int get hashCode => secondsSinceEpoch.hashCode;

  @override
  String toString() => 'Instant(${secondsSinceEpoch}s)';
}

/// Half-open window `[start, end)`. Construction asserts the invariant via
/// an initializer-list assertion, the production-standard guard shape.
class TimeWindow {
  final Instant start;
  final Instant end;

  TimeWindow({required this.start, required this.end})
    : assert(start.isBefore(end), 'window start must precede end');

  TimeWindow.startingAt(Instant start, Duration length)
    : this(start: start, end: start + length);

  bool contains(Instant moment) =>
      !moment.isBefore(start) && moment.isBefore(end);

  bool overlaps(TimeWindow other) =>
      start.isBefore(other.end) && other.start.isBefore(end);

  Duration get length =>
      Duration(seconds: end.secondsSinceEpoch - start.secondsSinceEpoch);

  /// The intersection window, or null when the windows are disjoint.
  TimeWindow? intersect(TimeWindow other) {
    final from = start.isAfter(other.start) ? start : other.start;
    final to = end.isBefore(other.end) ? end : other.end;
    if (from.isBefore(to)) {
      return TimeWindow(start: from, end: to);
    }
    return null;
  }

  @override
  String toString() => '[$start, $end)';
}
