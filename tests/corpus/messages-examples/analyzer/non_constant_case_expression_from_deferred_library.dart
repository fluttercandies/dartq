// messages-example: analyzer/messages.yaml CompileTimeErrorCode.nonConstantCaseExpressionFromDeferredLibrary
import 'a.dart' deferred as a;

void f(int x) {
  switch (x) {
    case a.zero:
      // ...
      break;
  }
}
