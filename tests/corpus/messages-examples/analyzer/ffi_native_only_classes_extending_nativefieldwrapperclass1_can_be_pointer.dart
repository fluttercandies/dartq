// messages-example: analyzer/messages.yaml FfiCode.ffiNativeOnlyClassesExtendingNativefieldwrapperclass1CanBePointer
import 'dart:ffi';

// MyService does not extend NativeFieldWrapperClass1
class MyService {
  @Native<Void Function(Pointer<Void>, Int8)>(symbol: 'MyService_process')
  external void process(int data);
}
