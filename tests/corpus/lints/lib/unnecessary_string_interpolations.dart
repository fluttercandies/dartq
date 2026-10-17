// unnecessary_string_interpolations — official rule faces
// (pkg/linter unnecessary_string_interpolations.dart:31-56): a string
// interpolation of exactly three elements — empty leading and trailing
// string segments around one expression — whose expression has static
// type String (non-nullable) reports at the whole literal. The
// interpolated element of an adjacent-strings chain never reports, a
// non-String expression keeps its interpolation, and a nullable String
// keeps it.
// Expected rows: 2 — UNNECESSARY_STRING_INTERPOLATIONS keyed
// unnecessary_string_interpolations.dart, message
// "Unnecessary use of string interpolation.":
//   `'$a'`   -> the sole `$identifier` interpolation of a String variable
//   `'${a}'` -> the brace form of the same face
void f() {
  var a = 'x';
  var b = '$a';
  var c = '${a}';
  print(b);
  print(c);
}

// GOOD: a non-empty leading segment keeps its interpolation, a non-String
// expression keeps it, and the interpolated element of an adjacent-strings
// chain never reports.
void g() {
  var d = 'x';
  var e = 'pre $d post';
  var h = '${d.length}';
  var i = 'y' '${d}';
  print(e);
  print(h);
  print(i);
}
