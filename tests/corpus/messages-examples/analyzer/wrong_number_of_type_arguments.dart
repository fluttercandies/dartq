// messages-example: analyzer/messages.yaml CompileTimeErrorCode.wrongNumberOfTypeArguments
class C<E> {}

void f(C<int, int> x) {}
