// Boundary reads-vs-writes: a plain assignment is only a write; a
// statement-position compound assignment or increment also warns (its read
// does not count); a consumed read or an if-null assignment stays silent.
void f() {
  var x; // warns: write never read
  x = 1;
  var y; // warns: statement compound assignment
  y += 1;
  var z; // warns: statement increment
  z++;
  var w; // silent: if-null assignment reads
  w ??= 2;
  var t = 0; // silent: consumed read
  print(++t);
}
