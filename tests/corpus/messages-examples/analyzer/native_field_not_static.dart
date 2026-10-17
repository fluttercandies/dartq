// messages-example: analyzer/messages.yaml FfiCode.nativeFieldNotStatic
import 'dart:ffi';

class C {
  @Native<Int>()
  external int f;
}
