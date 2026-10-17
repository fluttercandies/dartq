// messages-example: analyzer/messages.yaml WarningCode.invalidSealedAnnotation
import 'package:meta/meta.dart';

class A {
  @sealed
  void m() {}
}
