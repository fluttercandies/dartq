// messages-example: analyzer/messages.yaml FfiCode.mustReturnVoid
import 'dart:ffi';

int f(int i) => i * 2;

void g() {
  NativeCallable<Int32 Function(Int32)>.listener(f);
}
