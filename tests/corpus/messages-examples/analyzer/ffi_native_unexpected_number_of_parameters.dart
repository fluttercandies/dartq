// messages-example: analyzer/messages.yaml FfiCode.ffiNativeUnexpectedNumberOfParameters
import 'dart:ffi';

@Native<Void Function(Double)>(symbol: 'f')
external void f(double x, double y);
