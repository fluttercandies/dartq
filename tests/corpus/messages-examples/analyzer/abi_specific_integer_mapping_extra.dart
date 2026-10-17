// messages-example: analyzer/messages.yaml FfiCode.abiSpecificIntegerMappingExtra
import 'dart:ffi';

@AbiSpecificIntegerMapping({Abi.macosX64: Int8()})
@AbiSpecificIntegerMapping({Abi.linuxX64: Uint16()})
final class C extends AbiSpecificInteger {
  const C();
}
