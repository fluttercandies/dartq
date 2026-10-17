// messages-example: analyzer/messages.yaml WarningCode.invalidLiteralAnnotation
import 'package:meta/meta.dart';

class C {
  @literal
  C();
}
