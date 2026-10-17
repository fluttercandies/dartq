// messages-example: linter/messages.yaml LinterLintCode.noDefaultCases (variant 2)
class const C._(final int i) {
  static const C a = ._(1);
  static const C b = ._(2);
  static const C c = ._(3);
}

void f(C c) {
  switch (c) {
    case .a:
      print('a');
    default:
      print('other');
  }
}
