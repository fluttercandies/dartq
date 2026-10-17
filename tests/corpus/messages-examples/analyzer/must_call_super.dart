// messages-example: analyzer/messages.yaml WarningCode.mustCallSuper
import 'package:meta/meta.dart';

class A {
  @mustCallSuper
  void m() {}
}

class B extends A {
  @override
  void m() {}
}
