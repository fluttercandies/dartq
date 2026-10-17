// Version-gated error at language version 3.6: wildcard (non-binding)
// formals ship at 3.7, so here BOTH `_` parameters bind and the second
// declaration collides with the first. The same shape under apps/shell
// (3.7) is the positive control (apps/shell/lib/wildcards.dart).
//
// Expectation (oracle at 3.6): duplicate_definition on the second `_`
// parameter.

int emit(int _, int _) => 7;

void main() {
  print(emit(1, 2));
}
