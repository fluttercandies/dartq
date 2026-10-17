// messages-example: analyzer/messages.yaml WarningCode.invalidFactoryMethodImpl
import 'package:meta/meta.dart';

class Factory {
  C c = C();

  @factory
  C createC() => c;
}

class C {}
