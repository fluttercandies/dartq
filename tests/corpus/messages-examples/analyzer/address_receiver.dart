// messages-example: analyzer/messages.yaml FfiCode.addressReceiver
import 'dart:ffi';

final class MyStruct extends Struct {
  @Uint8()
  external int x;

  @Uint8()
  external int y;
}

@Native<Void Function(Pointer)>(isLeaf: true)
external void nativeLeafCall(Pointer ptr);

void main() {
  final struct = Struct.create<MyStruct>();
  final y = struct.y;
  // Incorrect: The receiver is not a struct field, but some integer.
  nativeLeafCall(y.address);
}
