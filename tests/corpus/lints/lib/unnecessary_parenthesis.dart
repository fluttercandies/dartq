// unnecessary_parenthesis — official rule faces
// (pkg/linter unnecessary_parenthesis.dart:39-232): a parenthesized
// expression reports when removing the parentheses keeps the meaning —
// the single-positional-argument parent, the one-token inner, and the
// null-aware chain inner report; operator parents, conditions, cascades,
// and directly-invoked parens are among the ported bail faces.
// Expected rows: 7 — UNNECESSARY_PARENTHESIS keyed
// unnecessary_parenthesis.dart, message
// "Unnecessary use of parentheses.":
//   the `(a)` single positional argument of `g`
//   the `((3,))` argument of `rec((int,) i)` — a record-literal inner
//   the `(3)` argument of `plain(int i)` — the record exemption misses
//   the `(tearoff<int>)` tear-off inner of `.hashCode`
//   the `(s)` of `(s).t` — a non-nullable extension keeps the report
//   the `(t)` of `(t).g` — the `String` on-type member is more specific
//   the `(t)` of `(t).m()` — the same split over the method face
import 'unnecessary_parenthesis_ext.dart';

void g(int a) {}

void f(int a) {
  g((a));

  // GOOD: an operator operand keeps its parenthesization.
  var x = (a + 1) * 2;
}

// #1346 faces — the record-type exemptions (pkg/linter
// unnecessary_parenthesis.dart:68-86): a parenthesized variable
// initializer or argument whose written type is a record type keeps
// the parentheses while the inner is not a record literal — the
// variable face (:69-72), the positional-argument face (:75-80), and
// the named-argument face (:83-86).
void rec((int,) i) {}

void named({required (int,) i}) {}

void plain(int i) {}

void recordFaces() {
  // GOOD: the record-typed variable keeps the parens (:69-72).
  (int,) r = (3);
  // GOOD: the positional record formal keeps them (:75-80).
  rec((3));
  // GOOD: the named record formal keeps them (:83-86).
  named(i: (3));
  // A record-literal inner loses the exemption; the single-argument
  // parent (:88-100) reports.
  rec(((3,)));
  // An int formal misses the exemption; the same parent reports.
  plain((3));
}

// The type-literal inner face (pkg/linter unnecessary_parenthesis.dart
// :105-106): `(name<...>)` around a type literal keeps the parentheses;
// a same-file generic tear-off is a function reference, not a type
// literal, and falls through to the report (:270).
int tearoff<T>(T x) => 0;

void typeLiteralFaces() {
  // GOOD: the type-literal inner keeps the parens (:105-106).
  (List<int>).toString();
  // A tear-off inner reports.
  (tearoff<int>).hashCode;
}

// The extension-nullable bail (pkg/linter unnecessary_parenthesis.dart
// :117-124 and :132-139): the parentheses stay when the accessor
// resolves to a same-file extension member whose `on` type is written
// nullable — they are what stops the null-aware shorting; a
// non-nullable `on` type keeps the one-token report (:108-161).
class C {
  String? get s => 'yay';
}

extension on String? {
  bool get g => false;

  bool m() => false;
}

extension on String {
  bool get t => false;
}

void extensionFaces(C? c, String s) {
  // GOOD: the nullable-extension getter keeps the parens (:117-124).
  (c?.s).g;
  // GOOD: the nullable-extension method keeps them (:132-139).
  (c?.s).m();
  // A non-nullable `on` type keeps the one-token report (:108-161).
  (s).t;
}

// The imported-extension face (pkg/linter unnecessary_parenthesis.dart
// :120-124): the accessor resolves through the import above to
// `NullableStringExt.g`, whose `on` type is written nullable — the
// same keep as the same-file declaration, on both sides.
void importedExtensionFaces(String? s) {
  // GOOD: the nullable extension arrives through the import (:120-124).
  (s).g;
}

// The dual-nullability face (pkg/linter unnecessary_parenthesis.dart
// :117-139): two same-named members judge independently through the
// resolved receiver — a `String?` receiver only fits the nullable
// on-type member and keeps the parens (:117-124, :132-139); a `String`
// receiver fits both and the non-nullable on-type member is the more
// specific resolution, so the one-token report stands (:108-161).
extension NullableG on String? {
  bool get g => false;

  bool m() => false;
}

extension NonNullableG on String {
  bool get g => false;

  bool m() => false;
}

void dualNullabilityFaces(String? s, String t) {
  // GOOD: the receiver is `String?` — only the nullable member applies.
  (s).g;
  (s).m();
  // The receiver is `String` — the non-nullable member is the more
  // specific resolution and the one-token report stands (:108-161).
  (t).g;
  (t).m();
}
