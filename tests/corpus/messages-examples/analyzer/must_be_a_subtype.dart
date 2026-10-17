// messages-example: analyzer/messages.yaml FfiCode.mustBeASubtype
import 'dart:ffi';

typedef T = Int8 Function(Int8);

double f(double i) => i;

void g() {
  Pointer.fromFunction<T>(f, 5.0);
}
