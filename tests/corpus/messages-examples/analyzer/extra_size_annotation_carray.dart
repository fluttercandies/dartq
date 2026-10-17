// messages-example: analyzer/messages.yaml FfiCode.extraSizeAnnotationCarray
import 'dart:ffi';

final class C extends Struct {
  @Array(4)
  @Array(8)
  external Array<Uint8> a0;
}
