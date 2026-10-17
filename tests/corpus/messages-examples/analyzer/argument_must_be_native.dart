// messages-example: analyzer/messages.yaml FfiCode.argumentMustBeNative
import 'dart:ffi';

@Native<Void Function()>()
external void f();

void g() {
  print(Native.addressOf('f'));
}
