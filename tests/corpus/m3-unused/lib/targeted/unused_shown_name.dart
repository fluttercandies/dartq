// Targets unused_shown_name: a name brought in by a 'show' combinator but
// never referenced — A is used below, B is not. Companion:
// unused_shown_name_other.dart exports both names.
import 'unused_shown_name_other.dart' show A, B;

void f() {
  A();
}
