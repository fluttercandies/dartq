// messages-example: analyzer/messages.yaml WarningCode.invalidOverrideOfNonVirtualMember
import 'package:meta/meta.dart';

class A {
  @nonVirtual
  void m() {}
}

class B extends A {
  @override
  void m() {}
}
