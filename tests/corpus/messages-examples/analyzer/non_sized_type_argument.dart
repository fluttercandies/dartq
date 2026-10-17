// messages-example: analyzer/messages.yaml FfiCode.nonSizedTypeArgument
import 'dart:ffi';

final class C extends Struct {
  @Array(8)
  external Array<Void> a0;
}
