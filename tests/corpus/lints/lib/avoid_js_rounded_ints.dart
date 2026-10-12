// avoid_js_rounded_ints — official rule faces
// (pkg/linter avoid_js_rounded_ints.dart:24-41): integer literals that
// change value when compiled to JavaScript — i.e. beyond 2^53 exact —
// in decimal or hexadecimal spelling.
// Expected rows: 3 — AVOID_JS_ROUNDED_INTS keyed
// avoid_js_rounded_ints.dart, message
// "Integer literal can't be represented exactly when compiled to JavaScript.":
//   `9007199254740993`  -> 2^53 + 1 decimal
//   `0x20000000000001`  -> 2^53 + 1 hexadecimal
//   `-9007199254740993` -> the negated literal is itself out of range
void f() {
  var a = 9007199254740993;
  var b = 0x20000000000001;
  var c = -9007199254740993;
  print(a);
  print(b);
  print(c);
}

// GOOD: 2^53 itself and small literals are exactly representable.
void g() {
  var a = 9007199254740992;
  var b = 1;
  print(a);
  print(b);
}
