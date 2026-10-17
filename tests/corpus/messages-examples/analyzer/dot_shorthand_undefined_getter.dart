// messages-example: analyzer/messages.yaml CompileTimeErrorCode.dotShorthandUndefinedGetter
void f() {
  E e = .c;
  print(e);
}

enum E { a, b }
