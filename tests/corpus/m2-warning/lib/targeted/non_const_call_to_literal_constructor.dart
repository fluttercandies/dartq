// Targets non_const_call_to_literal_constructor: a non-const instance
// creation whose constructor carries the package:meta @literal annotation
// and whose arguments are all const-evaluatable must be 'const'
// (best_practices_verifier.dart, _checkForLiteralConstructorUse). q is the
// already-const negative; r is the non-const-argument negative.
import 'package:meta/meta.dart';

class Point {
  final int x, y;

  @literal
  const Point(this.x, this.y);
}

final p = Point(1, 2);
final q = const Point(3, 4);
final notConst = DateTime.now().millisecondsSinceEpoch;
final r = Point(notConst, 6);
