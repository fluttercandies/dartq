// Records x patterns x sealed interaction faces: exhaustive switch
// expressions over a sealed hierarchy, the switch-statement
// asymmetry, record payloads destructured by positional and named
// patterns, records as map keys, extensions on record types with a
// record bound, and the never-matching record pattern.
//
// Grounding (upstream Dart SDK source,
// pkg/analyzer/lib/src/diagnostic/diagnostic.g.dart):
// non_exhaustive_switch_expression — "The type '{0}' isn't exhaustively
// matched by the switch cases since it doesn't match the pattern
// '{1}'."; pattern_never_matches_value_type (STATIC_WARNING) — "The
// matched value type '{0}' can never match the required type '{1}'.",
// reported per pattern node whose required type is provably
// uninhabitable for the matched value (upstream Dart SDK source,
// pkg/analyzer/lib/src/dart/resolver/type_analyzer.dart,
// checkPatternNeverMatchesValueType).
//
// Expected rows: 3 —
//   non_exhaustive_switch_expression:
//     `lanes(event)` — the switch expression covers Picked and Scanned
//     but not Delivered, and only switch expressions demand
//     exhaustiveness over sealed types
//   pattern_never_matches_value_type:
//     `(int _, int _)` in `isPairOfInts` — the record pattern's
//     required type (int, int) is uninhabitable for the matched
//     (int, String)
//     the second field pattern `int _` of the same case — String never
//     matches the required int; the first field (int over int) stays
//     silent

sealed class ShipmentEvent {
  const ShipmentEvent();

  String get parcelId;
}

class Picked extends ShipmentEvent {
  @override
  final String parcelId;
  final String depot;

  Picked(this.parcelId, this.depot);
}

class Scanned extends ShipmentEvent {
  @override
  final String parcelId;
  final (int, String) reading;

  Scanned(this.parcelId, this.reading);
}

class Delivered extends ShipmentEvent {
  @override
  final String parcelId;
  final ({String signedBy, int attempts}) proof;

  Delivered(this.parcelId, this.proof);
}

// Silence face: the expression covers every subclass of the sealed
// hierarchy, so exhaustiveness holds without a wildcard.
String stage(ShipmentEvent event) => switch (event) {
      Picked(:final depot) => 'staged at $depot',
      Scanned(:final reading) => 'scanned, note ${reading.$2}',
      Delivered(:final proof) => 'signed by ${proof.signedBy}',
    };

// The row: the expression drops Delivered from a sealed hierarchy.
String lanes(ShipmentEvent event) => switch (event) {
      Picked() => 'lane-a',
      Scanned() => 'lane-b',
    };

// Silence face: switch statements carry no exhaustiveness requirement,
// so the same two-case coverage over the sealed type stays clean.
int laneCount(ShipmentEvent event) {
  var count = 0;
  switch (event) {
    case Picked():
      count = 1;
    case Scanned():
      count = 2;
  }
  return count;
}

// Records as extension receivers with a record bound: the extension
// members resolve on the plain record type, and the bounded generic
// propagates them through inference.
extension ReadingExt on (int, String) {
  int get celsius => $1;

  String get note => $2;
}

T coldest<T extends (int, String)>(List<T> readings) =>
    readings.reduce((a, b) => a.$1 <= b.$1 ? a : b);

void readingFaces() {
  var batch = <(int, String)>[
    (21, 'ambient'),
    (4, 'chilled'),
  ];
  print(batch.first.celsius);
  print(coldest(batch).note);
}

// Silence face: positional and named components destructure in one
// record pattern.
String breakdownMarker((int, {String zone}) reading) {
  var (celsius, zone: label) = reading;
  return '$label at $celsius';
}

// Silence face: records carry structural equality, so composite keys
// work as map keys without adapters.
final depotRoutes = <(String, int), String>{
  ('rotterdam', 4): 'north-sea',
  ('antwerp', 2): 'rhine',
};

String routeFor(String depot, int bay) =>
    depotRoutes[(depot, bay)] ?? 'unassigned';

// The rows: the record pattern and its second field pattern are both
// uninhabitable for the matched (int, String) value.
bool isPairOfInts((int, String) reading) => switch (reading) {
      (int _, int _) => true,
      _ => false,
    };

void shipmentFaces() {
  var route = routeFor('rotterdam', 4);
  print(route);
  print(breakdownMarker((9, zone: 'reefer')));
  print(isPairOfInts((3, 'haze')));
  print(stage(Picked('prc-1', 'gate-4')));
  print(lanes(Scanned('prc-2', (7, 'x-ray'))));
  print(laneCount(Delivered('prc-3', (signedBy: 'oye', attempts: 1))));
}
