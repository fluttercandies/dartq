// messages-example: analyzer/messages.yaml CompileTimeErrorCode.privateSetter
import 'a.dart';

void f() {
  A._f = 0;
}
