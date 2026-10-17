// messages-example: analyzer/messages.yaml FfiCode.negativeVariableDimension (variant 2)
import 'dart:ffi';

final class MyStruct2 extends Struct {
  @Array.variableMulti(variableDimension: -1, [1, 2])
  external Array<Array<Array<Uint8>>> a0;
}
