// messages-example: linter/messages.yaml LinterLintCode.unsafeVariance
class C<X> {
  final bool Function(X) f;
  C(this.f);
}
