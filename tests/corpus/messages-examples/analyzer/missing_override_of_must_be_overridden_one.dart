// messages-example: analyzer/messages.yaml WarningCode.missingOverrideOfMustBeOverriddenOne
import 'package:meta/meta.dart';

class A {
  @mustBeOverridden
  void m() {}
}

class B extends A {}
