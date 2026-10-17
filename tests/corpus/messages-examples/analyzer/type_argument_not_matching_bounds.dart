// messages-example: analyzer/messages.yaml CompileTimeErrorCode.typeArgumentNotMatchingBounds
class A<E extends num> {}

var a = A<String>();
