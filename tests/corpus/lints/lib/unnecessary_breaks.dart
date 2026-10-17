// unnecessary_breaks — official rule faces
// (pkg/linter unnecessary_breaks.dart): a non-labeled break that is the
// last of MORE than one statement in a switch member reports — a sole
// statement keeps its break, and the implicit fall-through of the
// pattern cases covers the rest. A labeled break is exempted.
// Expected rows: 1 — UNNECESSARY_BREAKS keyed unnecessary_breaks.dart,
// message "Unnecessary 'break' statement.":
//   the trailing break after `print(x)` in `case 1`
void f(int x) {
  switch (x) {
    case 1:
      print(x);
      break;
  }
}

// GOOD: a sole statement keeps its break, and a labeled break is exempt.
void g(int x) {
  switch (x) {
    case 1:
      break;
    case 2:
      loop:
      for (var i = 0; i < 1; i++) {
        if (i == 0) break loop;
      }
  }
}
