// messages-example: analyzer/messages.yaml FfiCode.missingAnnotationOnStructField
import 'dart:ffi';

final class C extends Struct {
  external int x;
}
