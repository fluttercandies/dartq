// messages-example: analyzer/messages.yaml FfiCode.leafCallMustNotTakeHandle
import 'dart:ffi';

void f(Pointer<NativeFunction<Void Function(Handle)>> p) {
  p.asFunction<void Function(Object)>(isLeaf: true);
}
