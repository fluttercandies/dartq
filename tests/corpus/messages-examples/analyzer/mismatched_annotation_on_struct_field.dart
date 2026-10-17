// messages-example: analyzer/messages.yaml FfiCode.mismatchedAnnotationOnStructField
import 'dart:ffi';

final class C extends Struct {
  @Double()
  external int x;
}
