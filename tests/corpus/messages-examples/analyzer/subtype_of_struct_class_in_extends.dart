// messages-example: analyzer/messages.yaml FfiCode.subtypeOfStructClassInExtends
import 'dart:ffi';

final class S extends Struct {
  external Pointer f;
}

final class C extends S {
  external Pointer g;
}
