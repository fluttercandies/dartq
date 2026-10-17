// messages-example: analyzer/messages.yaml FfiCode.abiSpecificIntegerInvalid (variant 4)
import 'dart:ffi';

@AbiSpecificIntegerMapping({Abi.macosX64: Int8()})
final class C extends AbiSpecificInteger {
  final int i;

  const C(this.i);
}
