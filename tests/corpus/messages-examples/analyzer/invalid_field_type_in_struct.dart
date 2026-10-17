// messages-example: analyzer/messages.yaml FfiCode.invalidFieldTypeInStruct
import 'dart:ffi';

final class C extends Struct {
  external String s;

  @Int32()
  external int i;
}
