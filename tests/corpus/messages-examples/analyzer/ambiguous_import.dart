// messages-example: analyzer/messages.yaml CompileTimeErrorCode.ambiguousImport
import 'a.dart';
import 'b.dart';

void f(A a, C c1, C c2) {}
