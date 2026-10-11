/// Storage bins addressed by zone/aisle/shelf. The class uses a PRIMARY
/// CONSTRUCTOR (stable since 3.13): the header parameters are the fields,
/// no explicit constructor or field declarations needed.
library;

/// A physical storage bin. `final String zone` etc. in the primary
/// constructor header declare final fields directly.
class Bin(final String zone, final int aisle, final int shelf) {
  String get code => '$zone-A${aisle.toString().padLeft(2, '0')}'
      '-S${shelf.toString().padLeft(2, '0')}';

  bool sameZoneAs(Bin other) => zone == other.zone;

  /// Manhattan distance across the floor grid, the picking heuristic.
  int walkingDistance(Bin other) =>
      (aisle - other.aisle).abs() + (shelf - other.shelf).abs() * 2;

  @override
  String toString() => 'Bin($code)';
}

/// Mutable staging buffer for one bin; a second primary constructor with
/// `var` fields shows the writable form.
class StagingSlot(final String zone, var int reservedUnits) {
  void release(int units) {
    if (reservedUnits >= units) {
      reservedUnits -= units;
    }
  }

  bool get isFree => reservedUnits == 0;
}
