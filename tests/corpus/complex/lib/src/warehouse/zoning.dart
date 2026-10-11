/// Floor zoning: dot shorthands (stable since 3.10) for enum-static access
/// and null-aware elements (stable since 3.8) in collection literals.
library;

enum ZoneLabel { receiving, staging, picking, packing, dispatch }

/// Default flow: the context type on the left of the assignment lets the
/// dot shorthand name the constant directly.
ZoneLabel defaultZone() {
  ZoneLabel zone = .picking;
  return zone;
}

/// Comparison against a shorthand; same context-type resolution.
bool isOutbound(ZoneLabel zone) => zone == .dispatch || zone == .packing;

/// Shorthand in argument position: the parameter type provides the context.
String zoneGlyph(ZoneLabel zone) => switch (zone) {
      ZoneLabel.receiving => 'IN',
      ZoneLabel.staging => 'ST',
      ZoneLabel.picking => 'PK',
      ZoneLabel.packing => 'PA',
      ZoneLabel.dispatch => 'OUT',
    };

/// Null-aware elements: nullable entries are skipped with a leading `?`.
List<ZoneLabel> activeZones(ZoneLabel? primary, ZoneLabel? backup) =>
    [
      ?primary,
      ?backup,
      .staging,
    ];

/// Null-aware map entries: the value side takes `?` against a nullable
/// value; the key side takes `?` against a genuinely nullable key (a
/// constant key can never be null, so `?` there is dead).
Map<String, ZoneLabel> zoneAliases({
  ZoneLabel? fastLane,
  String? slowKey,
}) =>
    {
      'fast': ?fastLane,
      ?slowKey: .staging,
      'hub': .dispatch,
    };

/// Aisle routing table; const map keyed by zone with digit-separated caps.
const Map<ZoneLabel, int> hourlyThroughputCap = {
  ZoneLabel.receiving: 12_000,
  ZoneLabel.staging: 9_500,
  ZoneLabel.picking: 18_000,
  ZoneLabel.packing: 15_250,
  ZoneLabel.dispatch: 11_000,
};

/// Cap lookup falling back to the most conservative lane; the shorthand's
/// context type comes from the local's declared type (the index parameter
/// of `operator []` is Object?, too weak to resolve the shorthand).
int throughputCap(ZoneLabel? zone) {
  final ZoneLabel effective = zone ?? .staging;
  return hourlyThroughputCap[effective] ?? 0;
}
