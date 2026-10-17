// messages-example: analyzer/messages.yaml FfiCode.leafCallMustNotReturnHandle
import 'dart:ffi';

void f(Pointer<NativeFunction<Handle Function()>> p) {
  p.asFunction<Object Function()>(isLeaf: true);
}
