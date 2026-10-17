// messages-example: analyzer/messages.yaml FfiCode.ffiNativeInvalidMultipleAnnotations
import 'dart:ffi';

@Native<Int32 Function(Int32)>()
@Native<Int32 Function(Int32)>(isLeaf: true)
external int f(int v);
