// messages-example: analyzer/messages.yaml WarningCode.missingRequiredParam
import 'package:meta/meta.dart';

// ignore: deprecated_member_use
void f({@required int? x}) {}

void g() {
  f();
}
