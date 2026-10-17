// messages-example: linter/messages.yaml LinterLintCode.preferConstConstructorsInImmutables
import 'package:meta/meta.dart';

@immutable
class C {
  final int f;

  C(this.f);
}
