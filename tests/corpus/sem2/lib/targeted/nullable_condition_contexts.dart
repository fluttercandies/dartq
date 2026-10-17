// Targets unchecked_use_of_nullable_value: a nullable expression used as a
// condition, across every condition context. Oracle message (nail-pinned):
// "A nullable expression can't be used as a condition."
//
// Expected: ~12 rows of unchecked_use_of_nullable_value, one per bare
// nullable operand at its evaluation point. Breakdown verified against
// upstream flow semantics (analyzer source): 7 whole-condition rows (if,
// collection-if, while, do-while, C-style for, assert, ?:), 4 operand rows
// (each operand of &&/|| is checked independently; the right operand resolves
// under the left's short-circuit branch), and 1 row for the parenthesized
// condition (typed through the parens). The four silences are the
// load-bearing negatives: right operands promoted by the left's null-test
// branch (p != null && p, q == null || q), a non-reference operand (g()),
// and a null comparison that is not a bare use (b == null; no
// unnecessary_null_comparison co-fire either, since parameters are never
// definitely unassigned). `l` is consumed to avoid an unused_local_variable
// co-fire. Exact counts are fixed by the N2 re-baseline leg.
void f(bool? b, bool? p, bool? q) {
  if (b) {}
  var l = [if (b) 1];
  print(l);
  while (b) {
    break;
  }
  do {
    print(0);
  } while (b);
  for (; b; ) {}
  assert(b);
  b ? 1 : 2;
  if (p && q) {}
  if (p || q) {}
  if (p != null && p) {}
  if (q == null || q) {}
  if ((b)) {}
  if (g()) {}
  if (b == null) {}
}

bool g() => true;
