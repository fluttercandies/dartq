// messages-example: analyzer/messages.yaml FfiCode.fieldInStructWithInitializer
import 'dart:ffi';

final class C extends Struct {
  Pointer p = nullptr;
}
