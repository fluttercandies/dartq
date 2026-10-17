// messages-example: linter/messages.yaml LinterLintCode.exhaustiveCases (variant 2)
extension type const E._(int value) {
  static const E a = ._(1);
  static const E b = ._(2);
}

void f(E e) {
  switch (e) {
    case .a:
      print('a');
  }
}
