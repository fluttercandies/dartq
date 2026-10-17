// messages-example: analyzer/messages.yaml CompileTimeErrorCode.undefinedPrefixedName
import 'dart:core' as p;

void f() {
  p.a;
}
