// messages-example: analyzer/messages.yaml FfiCode.nonConstantTypeArgument
import 'dart:ffi';

typedef T = int Function(int);

class C<R extends T> {
  void m(Pointer<NativeFunction<T>> p) {
    p.asFunction<R>();
  }
}
