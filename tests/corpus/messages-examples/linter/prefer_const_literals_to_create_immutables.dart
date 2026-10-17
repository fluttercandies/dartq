// messages-example: linter/messages.yaml LinterLintCode.preferConstLiteralsToCreateImmutables
import 'package:meta/meta.dart';

@immutable
class C {
  final List<int> f;

  const C(this.f);
}

C c = C([1]);
