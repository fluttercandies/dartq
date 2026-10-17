// messages-example: analyzer/messages.yaml FfiCode.argumentMustBeNative (variant 2)
import 'dart:ffi';

external void f();

void g() {
  print(Native.addressOf<NativeFunction<Void Function()>>(f));
}
