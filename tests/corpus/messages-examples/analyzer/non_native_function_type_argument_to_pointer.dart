// messages-example: analyzer/messages.yaml FfiCode.nonNativeFunctionTypeArgumentToPointer
import 'dart:ffi';

typedef FNative = int Function(int);
typedef F = int Function(int);

class C {
  void f(Pointer<NativeFunction<FNative>> p) {
    p.asFunction<F>();
  }
}
