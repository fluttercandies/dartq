// messages-example: analyzer/messages.yaml WarningCode.invalidUseOfVisibleForTestingMember
import 'c.dart';

void f(C c) {
  c.m();
}
