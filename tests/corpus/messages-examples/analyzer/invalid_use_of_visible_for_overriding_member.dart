// messages-example: analyzer/messages.yaml WarningCode.invalidUseOfVisibleForOverridingMember
import 'a.dart';

class B extends A {
  void b() {
    a();
  }
}
