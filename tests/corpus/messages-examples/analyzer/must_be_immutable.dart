// messages-example: analyzer/messages.yaml WarningCode.mustBeImmutable
import 'package:meta/meta.dart';

@immutable
class C {
  int x;

  C(this.x);
}
