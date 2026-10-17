// messages-example: analyzer/messages.yaml FfiCode.ffiNativeMustBeExternal
import 'dart:ffi';

@Native<Void Function(Pointer<Void>)>()
void free(Pointer<Void> ptr) {}
