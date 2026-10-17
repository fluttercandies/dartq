// messages-example: analyzer/messages.yaml FfiCode.creationOfStructOrUnion
import 'dart:ffi';

final class C extends Struct {
  @Int32()
  external int a;
}

void f() {
  C();
}
