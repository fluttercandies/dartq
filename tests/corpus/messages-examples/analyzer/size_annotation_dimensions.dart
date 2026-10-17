// messages-example: analyzer/messages.yaml FfiCode.sizeAnnotationDimensions
import 'dart:ffi';

final class C extends Struct {
  @Array(8, 8)
  external Array<Array<Array<Uint8>>> a0;
}
