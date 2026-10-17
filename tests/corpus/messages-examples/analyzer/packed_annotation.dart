// messages-example: analyzer/messages.yaml FfiCode.packedAnnotation
import 'dart:ffi';

@Packed(1)
@Packed(1)
final class C extends Struct {
  external Pointer<Uint8> notEmpty;
}
