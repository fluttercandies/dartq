// messages-example: analyzer/messages.yaml FfiCode.missingExceptionValue
import 'dart:ffi';

int f(int i) => i * 2;

void g() {
  Pointer.fromFunction<Int8 Function(Int8)>(f);
}
