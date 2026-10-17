// messages-example: linter/messages.yaml LinterLintCode.unnecessaryThisAlias
class C {
  void m() {
    var self = this;
    if (self is D) {
      // ...
    }
  }
}

class D extends C {}
