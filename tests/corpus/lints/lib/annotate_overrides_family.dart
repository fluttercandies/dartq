// annotate_overrides family — fixture #1347: one base hierarchy, every
// overridable member kind in primary-constructor and classic-body form.
// The rule rides the vendored recommended.yaml include, so no options
// entry. The base's operator== carries @override: its only override
// target would be Object's == (a cross-file ancestor), which stays out of
// shape — the same-file written supertype chain is the ported face. The
// @override annotations sit exactly where the face is a silence, so the
// firing faces below are left bare.
// Expected rows: 4, all keyed annotate_overrides_family.dart —
// ANNOTATE_OVERRIDES:
//   the primary-constructor field formal `x` of B (var int x, no `=`
//   suffix)
//   the getter `g` of D
//   the setter `s` of E (the message names `s=`)
//   the unannotated operator `==` of F
// OVERRIDDEN_FIELDS (recommended.yaml) co-fires at the B/C field formals
// overriding A's field — a symmetric both-sides row.
class A {
  int x = 0;

  int get g => 0;

  set s(int v) {}

  @override
  bool operator ==(Object o) => true;
}

class B(var int x) extends A;

class C(@override var int x) extends A;

class D extends A {
  int get g => 1;
}

class E extends A {
  set s(int v) {}
}

class F extends A {
  bool operator ==(Object o) => false;
}

class G extends A {
  @override
  bool operator ==(Object o) => false;
}
