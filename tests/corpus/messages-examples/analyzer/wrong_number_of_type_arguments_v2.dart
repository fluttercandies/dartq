// messages-example: analyzer/messages.yaml CompileTimeErrorCode.wrongNumberOfTypeArguments (variant 2)
class C<E> {}

var c = C<int, int>();
