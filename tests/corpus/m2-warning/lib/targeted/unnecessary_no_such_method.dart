// Targets unnecessary_no_such_method: a noSuchMethod declaration whose
// body is exactly `super.noSuchMethod(<one argument>)` and whose super
// dispatch resolves to a concrete declaration is unnecessary
// (best_practices_verifier.dart, _checkForUnnecessaryNoSuchMethod). C
// reports (super resolves to B's declaration); E is the in-file negative
// (its super resolves to Object).
class B {
  @override
  dynamic noSuchMethod(Invocation invocation) => 42;
}

class C extends B {
  @override
  dynamic noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation);
}

class E {
  @override
  dynamic noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation);
}
