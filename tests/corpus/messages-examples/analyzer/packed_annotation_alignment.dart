// messages-example: analyzer/messages.yaml FfiCode.packedAnnotationAlignment
import 'dart:ffi';

@Packed(3)
final class C extends Struct {
  external Pointer<Uint8> notEmpty;
}
