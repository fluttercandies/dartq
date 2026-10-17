// messages-example: analyzer/messages.yaml CompileTimeErrorCode.wrongNumberOfTypeArgumentsExtension
extension E<T> on List<T> {
  int get len => length;
}

void f(List<int> p) {
  E<int, String>(p).len;
}
