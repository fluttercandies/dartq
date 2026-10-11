// collection_methods_unrelated_type — ported rows only (audit r1 thin
// row): same-file typed fields reached through `this.` (the dartq pin
// face, collections.rs `contains_reports_mismatched_member_types`). The
// ledgered unported faces (`Map[]=` index assignment, cross-file Queue
// remove) and the opaque-receiver presumption are out of shape here.
// Expected rows: 4 — COLLECTION_METHODS_UNRELATED_TYPE keyed
// collection_methods.dart: List<int>.contains(String), Set<String>.remove
// (int), Map<int, String>.containsKey(String), Map<int, String>.
// containsValue(int). Co-fire: UNNECESSARY_THIS on the four `this.`
// receivers (lines 16-19; no imports in the file, so the official
// resolveNameInScope check is None and visitThisExpression reports).
class CollectionSites {
  List<int> list = <int>[];
  Set<String> set = <String>{};
  Map<int, String> map = <int, String>{};

  void sites() {
    this.list.contains('x');
    this.set.remove(1);
    this.map.containsKey('k');
    this.map.containsValue(1);
  }
}
