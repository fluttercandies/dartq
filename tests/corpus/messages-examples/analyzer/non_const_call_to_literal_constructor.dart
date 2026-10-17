// messages-example: analyzer/messages.yaml WarningCode.nonConstCallToLiteralConstructor
import 'package:meta/meta.dart';

class C {
  @literal
  const C();
}

C f() => C();
