// Boundary loops-and-patterns: an unused for-in loop variable and an unused
// for-init variable warn; a counter read by its own condition is silent; a
// pattern declaration is all-or-nothing - with no name used every bound name
// reports, one used name silences the whole declaration (batch semantics).
void f() {
  for (var i in [1, 2]) {} // warns: loop variable never read
  for (var j = 0; j < 2; j++) {} // silent: read in condition
  for (var k = 0;;) { break; } // warns: never read
  var (a, b) = (1, 2); // warns for a and for b: neither used
  var (c, d) = (3, 4); // silent: one used name spares the pair
  print(c);
}
