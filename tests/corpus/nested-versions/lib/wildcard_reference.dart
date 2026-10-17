// Version-gated error at language version 3.7: non-binding wildcard
// formals ship at 3.7, so both `_` parameters are wildcards here and the
// body reference to `_` has no declaration in scope. At 3.6 the same
// file is legal — both formals would bind and `=> _` would refer to the
// first one.
//
// Expectation (oracle at 3.7): undefined_identifier on the body `_`.

int pick(int _, int _) => _;

void main() {
  print(pick(1, 2));
}
