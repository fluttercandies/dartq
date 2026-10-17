// messages-example: analyzer/messages.yaml FfiCode.extraAnnotationOnStructField
import 'dart:ffi';

final class C extends Struct {
  @Int32()
  @Int16()
  external int x;
}
