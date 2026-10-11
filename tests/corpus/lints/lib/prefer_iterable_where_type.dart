// prefer_iterable_whereType — audit r1 blind row. The ported face is the
// `where` + is-test closure (dartq pin
// where_type_reports_a_type_test_closure; the official
// prefer_iterable_whereType_test.dart test_whereType shape). Note the
// plan sketch's `.map((e) => e as T)` shape is not the rule's face.
// Expected rows: 1 — PREFER_ITERABLE_WHERETYPE keyed
// prefer_iterable_where_type.dart.
void f(List<int> xs) {
  xs.where((e) => e is int);
}
