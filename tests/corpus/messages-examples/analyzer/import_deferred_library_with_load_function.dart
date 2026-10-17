// messages-example: analyzer/messages.yaml HintCode.importDeferredLibraryWithLoadFunction
import 'a.dart' deferred as a;

void f() {
  a.Library();
}
