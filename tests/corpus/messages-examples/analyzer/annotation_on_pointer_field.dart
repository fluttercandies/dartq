// messages-example: analyzer/messages.yaml FfiCode.annotationOnPointerField
import 'dart:ffi';

final class C extends Struct {
  @Double()
  external Pointer<Int8> p;
}
