// Second half of the `circular_dependency` trigger: imports back to
// `circular_dependency_a.dart`, closing the cycle.
import 'circular_dependency_a.dart';

class CircularB {}
