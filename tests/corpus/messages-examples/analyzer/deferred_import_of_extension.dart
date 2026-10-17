// messages-example: analyzer/messages.yaml CompileTimeErrorCode.deferredImportOfExtension
import 'a.dart' deferred as a;

void f() {
  a.C();
}
