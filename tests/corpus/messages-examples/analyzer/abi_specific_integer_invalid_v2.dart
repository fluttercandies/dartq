// messages-example: analyzer/messages.yaml FfiCode.abiSpecificIntegerInvalid (variant 2)
import 'dart:ffi';

@AbiSpecificIntegerMapping({Abi.macosX64: Int8()})
final class C extends AbiSpecificInteger {
  C();
}
