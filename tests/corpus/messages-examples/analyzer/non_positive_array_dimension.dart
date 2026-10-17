// messages-example: analyzer/messages.yaml FfiCode.nonPositiveArrayDimension
import 'dart:ffi';

final class MyStruct extends Struct {
  @Array(-8)
  external Array<Uint8> a0;
}
