// messages-example: analyzer/messages.yaml FfiCode.genericStructSubclass
import 'dart:ffi';

final class S<T> extends Struct {
  external Pointer notEmpty;
}
