// messages-example: analyzer/messages.yaml FfiCode.negativeVariableDimension
import 'dart:ffi';

final class MyStruct extends Struct {
  @Array.variableWithVariableDimension(-1)
  external Array<Uint8> a0;
}
