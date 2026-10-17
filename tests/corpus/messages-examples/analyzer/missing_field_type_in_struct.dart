// messages-example: analyzer/messages.yaml FfiCode.missingFieldTypeInStruct
import 'dart:ffi';

final class C extends Struct {
  external var str;

  @Int32()
  external int i;
}
