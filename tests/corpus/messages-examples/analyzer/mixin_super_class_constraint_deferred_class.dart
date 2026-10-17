// messages-example: analyzer/messages.yaml CompileTimeErrorCode.mixinSuperClassConstraintDeferredClass
import 'dart:async' deferred as async;

mixin M<T> on async.Stream<T> {}
