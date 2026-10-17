// messages-example: analyzer/messages.yaml FfiCode.mustBeANativeFunctionType
import 'dart:ffi';

int f(int i) => i * 2;

class C<T extends Function> {
  void g() {
    Pointer.fromFunction<T>(f, 0);
  }
}
