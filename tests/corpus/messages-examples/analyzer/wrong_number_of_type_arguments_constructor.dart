// messages-example: analyzer/messages.yaml CompileTimeErrorCode.wrongNumberOfTypeArgumentsConstructor
class C<T> {
  C.named();
}

C f() => C.named<String>();
