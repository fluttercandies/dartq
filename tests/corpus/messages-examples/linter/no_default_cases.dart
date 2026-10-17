// messages-example: linter/messages.yaml LinterLintCode.noDefaultCases
enum E { a, b, c }

void f(E e) {
  switch (e) {
    case .a:
      print('a');
    default:
      print('other');
  }
}
