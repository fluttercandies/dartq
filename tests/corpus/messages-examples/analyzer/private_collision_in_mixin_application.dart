// messages-example: analyzer/messages.yaml CompileTimeErrorCode.privateCollisionInMixinApplication
import 'a.dart';

class C extends Object with A, B {}
