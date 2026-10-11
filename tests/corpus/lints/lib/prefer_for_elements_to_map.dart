// prefer_for_elements_to_map_fromIterable — audit r1 blind row. The
// canonical Map.fromIterable + two single-expression closures shape
// (dartq pin source at collections.rs; official test shape).
// Expected rows: 1 — PREFER_FOR_ELEMENTS_TO_MAP_FROMITERABLE keyed
// prefer_for_elements_to_map.dart.
void f(List<int> xs) {
  final m = Map.fromIterable(xs, key: (e) => e, value: (e) => e);
  print(m);
}
