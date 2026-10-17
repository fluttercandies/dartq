// messages-example: analyzer/messages.yaml FfiCode.abiSpecificIntegerInvalid (variant 5)
import 'dart:ffi';

@AbiSpecificIntegerMapping({Abi.macosX64: Int8()})
// type parameters
final class C<T> extends AbiSpecificInteger {
  const C();
}
