// tighten_type_of_initializing_formals — differential corpus (#1160).
// Source: upstream Dart SDK source,
// pkg/linter/test/rules/tighten_type_of_initializing_formals_test.dart
// (transcribed 2026-10-14, vendored SDK 3.15 main face).
// Inventory: 5 POSITIVE + 5 NEGATIVE = all 10 upstream tests, none
// skipped. The `new(...)` declaring-constructor spelling and the primary
// -constructor faces ride the primary-constructors feature (enabled at the
// corpus language version 3.13). Upstream's /*[n*/.../*n]*/ offset markers
// are normalized away; the marked sites are named in the block comments.
// Expected rows (oracle leg): 7 — TIGHTEN_TYPE_OF_INITIALIZING_FORMALS
// keyed tighten_type_of_initializing_formals.dart (1+2+1+2+1 across the
// POSITIVE blocks).

// POSITIVE upstream test_superInit (1 row at B's `super.a`).
class TTBase {
  String? a;
  TTBase(this.a);
}

class TTSub extends TTBase {
  TTSub(String super.a);
}

class TTSubAssert extends TTBase {
  TTSubAssert(super.a) : assert(a != null);
}

// POSITIVE upstream test_thisInit_asserts (2 rows at the two `this.p`
// formals).
class TTAsserts {
  String? p;
  TTAsserts(this.p) : assert(p != null);
  TTAsserts.a(this.p) : assert(null != p);
}

// POSITIVE upstream test_thisInit_asserts_newSyntax (1 row at `this.p` in
// the declaring constructor spelled `new`).
class TTNewSyntax {
  String? p;
  TTNewSyntax(this.p) : assert(p != null);
}

// POSITIVE upstream test_thisInit_asserts_positionalParams (2 rows: p1 and
// p4).
class TTPositional {
  TTPositional(
    this.p1,
    String? p2,
    this.p3, {
    this.p4,
    this.p5,
  }) : assert(p1 != null),
       assert(p2 != null),
       assert(p4 != null);

  String? p;
  String? p1;
  String? p2;
  String? p3;
  String? p4;
  String? p5;
}

// POSITIVE upstream test_thisInit_asserts_primaryConstructor (1 row at
// `this.p`; primary-constructors face).
class TTPrimaryAsserts(this.p) {
  String? p;
  this : assert(p != null);
}

// NEGATIVE upstream test_thisInit_noAssert.
class TTNoAssert {
  String? p;
  TTNoAssert(this.p);
}

// NEGATIVE upstream test_thisInit_noAssert_primaryConstructor
// (primary-constructors face).
class TTNoAssertPrimary(this.p) {
  String? p;
}

// NEGATIVE upstream test_thisInit_noAssert_primaryConstructor_declaring
// (primary-constructors face).
class TTNoAssertDeclaring(final String? p);

// NEGATIVE upstream test_thisInit_asserts_primaryConstructor_declaring
// (primary-constructors face; a declaring parameter is not an initializing
// formal, so the assert does not make it reportable).
class TTAssertsDeclaring(final String? p) {
  this : assert(p != null);
}

// NEGATIVE upstream test_thisInit_tightens: `String this.p` already names
// the tightened type, so no lint (upstream expects only two null-comparison
// errors, outside the lint filter).
class TTAlreadyTight {
  String? p;
  TTAlreadyTight(String this.p) : assert(p != null);
  TTAlreadyTight.a(String this.p) : assert(null != p);
}
