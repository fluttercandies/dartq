// messages-example: analyzer/messages.yaml FfiCode.nativeFieldInvalidType
import 'dart:ffi';

@Native<NativeFunction<Void Function()>>()
external void Function() free;
