// messages-example: analyzer/messages.yaml FfiCode.compoundImplementsFinalizable
import 'dart:ffi';

final class S extends Struct implements Finalizable {
  external Pointer notEmpty;
}
