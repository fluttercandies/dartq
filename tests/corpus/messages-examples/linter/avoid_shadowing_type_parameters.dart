// messages-example: linter/messages.yaml LinterLintCode.avoidShadowingTypeParameters
class C<T> {
  void m<T>() {}
}
