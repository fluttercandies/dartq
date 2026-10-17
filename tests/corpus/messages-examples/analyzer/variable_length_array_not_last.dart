// messages-example: analyzer/messages.yaml FfiCode.variableLengthArrayNotLast
import 'dart:ffi';

final class C extends Struct {
  @Array.variable()
  external Array<Uint8> a0;

  @Uint8()
  external int a1;
}
