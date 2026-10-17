// noop_primitive_operations — official rule faces
// (pkg/linter noop_primitive_operations.dart:29-105): three noop families
// report — a middle empty literal of an adjacent-strings group (the ends
// pass), a `target.toString()` with no arguments and a non-super target
// inside a string interpolation, and the typed invocations
// (`String.toString()` on a non-nullable String receiver, the int
// invariants toInt/round/ceil/floor/truncate, `double.toDouble()`); with
// no target, the core print face descends into its single argument.
// Expected rows: 9 — NOOP_PRIMITIVE_OPERATIONS keyed
// noop_primitive_operations.dart, message
// "The expression has no effect and can be removed.":
//   `x.toInt()`, `x.round()`, `x.ceil()`, `x.floor()`, `x.truncate()`
//   (the int invariants on an int receiver)
//   `s.toString()` (non-nullable String receiver)
//   `d.toDouble()` (double receiver)
//   the middle `''` of the adjacent-strings group
//   the `o.toString()` inside `print(o.toString())` (print descent)
void f(int x, String s, double d, Object o) {
  var a = x.toInt();
  var b = x.round();
  var c = x.ceil();
  var e = x.floor();
  var g = x.truncate();
  var h = s.toString();
  var i = d.toDouble();
  var j = 'a' '' 'b';
  print(o.toString());
}

// GOOD: a nullable receiver, a null-aware access, a num receiver, a
// dynamic receiver, a super target, and non-empty ends pass.
void g(String? maybe, num n, dynamic dyn) {
  var m1 = maybe.toString();
  var m2 = maybe?.toString();
  var m3 = n.toInt();
  var m4 = dyn.toString(1);
  var m5 = 'a' 'b' '';
}

class C {
  @override
  String toString() => 'C';

  String m() => super.toString();
}
