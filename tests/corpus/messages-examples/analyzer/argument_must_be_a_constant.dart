// messages-example: analyzer/messages.yaml FfiCode.argumentMustBeAConstant
import 'dart:ffi';

int Function(int) fromPointer(
  Pointer<NativeFunction<Int8 Function(Int8)>> p,
  bool isLeaf,
) {
  return p.asFunction(isLeaf: isLeaf);
}
