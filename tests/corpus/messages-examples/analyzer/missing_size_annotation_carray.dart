// messages-example: analyzer/messages.yaml FfiCode.missingSizeAnnotationCarray
import 'dart:ffi';

final class C extends Struct {
  external Array<Uint8> a0;
}
