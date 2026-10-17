// messages-example: analyzer/messages.yaml WarningCode.unusedResult
import 'package:meta/meta.dart';

class C {
  @useResult
  int a() => 0;

  int b() => 0;
}

void f(C c) {
  c.a();
}
