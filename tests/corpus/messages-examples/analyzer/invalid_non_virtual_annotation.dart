// messages-example: analyzer/messages.yaml WarningCode.invalidNonVirtualAnnotation
import 'package:meta/meta.dart';

abstract class C {
  @nonVirtual
  void m();
}
