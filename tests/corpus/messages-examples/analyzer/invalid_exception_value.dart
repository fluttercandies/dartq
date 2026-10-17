// messages-example: analyzer/messages.yaml FfiCode.invalidExceptionValue
import 'dart:ffi';

typedef T = Void Function(Int8);

void f(int i) {}

void g() {
  Pointer.fromFunction<T>(f, 42);
}
