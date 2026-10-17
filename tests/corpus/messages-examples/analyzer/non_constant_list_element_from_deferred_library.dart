// messages-example: analyzer/messages.yaml CompileTimeErrorCode.nonConstantListElementFromDeferredLibrary
import 'a.dart' deferred as a;

var l = const [a.zero];
