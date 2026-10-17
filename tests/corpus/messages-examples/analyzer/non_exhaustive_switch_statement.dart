// messages-example: analyzer/messages.yaml CompileTimeErrorCode.nonExhaustiveSwitchStatement
enum E { one, two, three }

void f(E e) {
  switch (e) {
    case E.one:
    case E.two:
  }
}
