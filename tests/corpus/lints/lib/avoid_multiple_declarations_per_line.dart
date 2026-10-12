// avoid_multiple_declarations_per_line — official rule faces
// (pkg/linter avoid_multiple_declarations_per_line.dart:30-49): a
// declaration list with more than one variable reports each variable
// after the first, for locals and fields alike; for-loop initializer
// lists are exempt.
// Expected rows: 3 — AVOID_MULTIPLE_DECLARATIONS_PER_LINE keyed
// avoid_multiple_declarations_per_line.dart:
//   `var a = 1, b = 2;`  -> row at `b`
//   `int c = 3, d = 4;`  -> row at `d`
//   `int x = 1, y = 2;`  -> row at `y` (field form)
var a = 1, b = 2;

int c = 3, d = 4;

class A {
  int x = 1, y = 2;
}

void f() {
  var e = 5;
  int g = 6;
  for (var i = 0, j = 1; i < 1; i++) {
    print(e + g + i + j);
  }
}
