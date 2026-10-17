// messages-example: analyzer/messages.yaml CompileTimeErrorCode.dotShorthandMissingContext
void f() {
  var e = .a;
  print(e);
}

enum E { a, b }
