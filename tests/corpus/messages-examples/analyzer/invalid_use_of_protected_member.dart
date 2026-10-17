// messages-example: analyzer/messages.yaml WarningCode.invalidUseOfProtectedMember
import 'a.dart';

void b(A a) {
  a.a();
}
