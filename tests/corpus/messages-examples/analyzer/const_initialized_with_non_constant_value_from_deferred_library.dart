// messages-example: analyzer/messages.yaml CompileTimeErrorCode.constInitializedWithNonConstantValueFromDeferredLibrary
import 'dart:math' deferred as math;

const pi = math.pi;
