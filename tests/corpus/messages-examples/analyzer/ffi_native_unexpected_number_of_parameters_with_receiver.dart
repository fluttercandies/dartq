// messages-example: analyzer/messages.yaml FfiCode.ffiNativeUnexpectedNumberOfParametersWithReceiver
import 'dart:ffi';

class C {
  @Native<Void Function(Double)>()
  external void f(double x);
}
