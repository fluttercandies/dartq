// messages-example: analyzer/messages.yaml FfiCode.abiSpecificIntegerMappingUnsupported
import 'dart:ffi';

@AbiSpecificIntegerMapping({Abi.macosX64: Array<Uint8>(4)})
final class C extends AbiSpecificInteger {
  const C();
}
