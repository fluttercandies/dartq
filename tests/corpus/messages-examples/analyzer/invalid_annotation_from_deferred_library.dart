// messages-example: analyzer/messages.yaml CompileTimeErrorCode.invalidAnnotationFromDeferredLibrary
import 'dart:math' deferred as math;

@math.pi
void f() {}
