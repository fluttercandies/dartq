// messages-example: analyzer/messages.yaml FfiCode.fieldInitializerInStruct
import 'dart:ffi';

final class C extends Struct {
  @Int32()
  int f;

  C() : f = 0;
}
