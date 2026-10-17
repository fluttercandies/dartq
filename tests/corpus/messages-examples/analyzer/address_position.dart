// messages-example: analyzer/messages.yaml FfiCode.addressPosition
import 'dart:ffi';
import 'dart:typed_data';

@Native<Void Function(Pointer<Uint8>)>()
external void nonLeafCall(Pointer<Uint8> ptr);

void main() {
  final data = Uint8List(10);

  // Incorrect: Using '.address' as an argument to a non-leaf call.
  nonLeafCall(data.address);
}
