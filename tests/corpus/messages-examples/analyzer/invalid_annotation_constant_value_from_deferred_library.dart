// messages-example: analyzer/messages.yaml CompileTimeErrorCode.invalidAnnotationConstantValueFromDeferredLibrary
import 'dart:math' deferred as math;

class C {
  const C(double d);
}

@C(math.pi)
void f() {}
