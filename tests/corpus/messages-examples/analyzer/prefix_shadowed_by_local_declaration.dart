// messages-example: analyzer/messages.yaml CompileTimeErrorCode.prefixShadowedByLocalDeclaration
import 'dart:async' as a;

a.Future? f(int a) {
  a.Future? x;
  return x;
}
