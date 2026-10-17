// messages-example: analyzer/messages.yaml WarningCode.invalidFactoryMethodDecl
import 'package:meta/meta.dart';

class Factory {
  @factory
  void createC() {}
}

class C {}
