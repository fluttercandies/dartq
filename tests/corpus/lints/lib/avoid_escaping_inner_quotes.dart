// avoid_escaping_inner_quotes — official rule faces
// (pkg/linter avoid_escaping_inner_quotes.dart:30-67): a non-raw,
// non-multiline string literal that escapes its own quote character
// while containing no other escapable character should swap the outer
// quotes instead. Both-quote literals, raw and multiline literals, and
// empty literals are exempt.
// Expected rows: 3 — AVOID_ESCAPING_INNER_QUOTES keyed
// avoid_escaping_inner_quotes.dart:
//   `'a\'b'`                    -> "Unnecessary escape of '''."
//                                  correction "Try changing the outer quotes to '\"'."
//   `'a\'b\'c'`                 -> same variants
//   `'interpolated \'$a\' tail'`-> same variants
void f() {
  var a = 'a\'b';
  var b = 'a\'b\'c';
  var c = 'interpolated \'$a\' tail';
  print(a);
  print(b);
  print(c);
}

// GOOD: outer quotes swapped, both quote kinds present, raw, multiline,
// and empty literals.
void g() {
  var d = "a'b";
  var e = 'a"b\'c';
  var raw = r'a"b';
  var multiline = '''
it's''';
  var empty = '';
  print(d);
  print(e);
  print(raw);
  print(multiline);
  print(empty);
}
