// messages-example: analyzer/messages.yaml WarningCode.redeclareOnNonRedeclaringMember
import 'package:meta/meta.dart';

class C {
  void m() {}
}

extension type E(C c) implements C {
  @redeclare
  void n() {}
}
