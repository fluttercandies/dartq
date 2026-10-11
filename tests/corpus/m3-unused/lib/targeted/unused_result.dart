// Targets unused_result: a value from a @useResult-annotated function
// discarded as an expression statement (package:meta supplies the
// annotation).
import 'package:meta/meta.dart';

@useResult
int f() => 1;

void g() {
  f();
}
