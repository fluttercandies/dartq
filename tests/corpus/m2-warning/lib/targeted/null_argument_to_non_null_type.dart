// Targets null_argument_to_non_null_type: Completer.complete over a
// non-nullable type argument rejects a null argument (NullSafeApiVerifier).
import 'dart:async';

void f(Completer<int> c) {
  c.complete(null);
}
