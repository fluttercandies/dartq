// messages-example: analyzer/messages.yaml WarningCode.unnecessaryNoSuchMethod
class A {
  @override
  dynamic noSuchMethod(invocation) => super.noSuchMethod(invocation);
}

class B extends A {
  @override
  dynamic noSuchMethod(invocation) {
    return super.noSuchMethod(invocation);
  }
}
