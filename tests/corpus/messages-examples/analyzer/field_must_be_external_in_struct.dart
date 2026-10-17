// messages-example: analyzer/messages.yaml FfiCode.fieldMustBeExternalInStruct
import 'dart:ffi';

final class C extends Struct {
  @Int16()
  int a;
}
