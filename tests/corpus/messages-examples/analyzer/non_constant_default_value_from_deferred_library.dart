// messages-example: analyzer/messages.yaml CompileTimeErrorCode.nonConstantDefaultValueFromDeferredLibrary
import 'a.dart' deferred as a;

void f({int x = a.zero}) {}
