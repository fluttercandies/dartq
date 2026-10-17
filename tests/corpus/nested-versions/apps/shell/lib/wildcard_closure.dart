// Version-gated error at language version 3.7, closure-parameter form
// of the wildcard flip: the two `_` parameters are non-binding
// wildcards at 3.7, so the closure body's `_` has nothing in scope. At
// 3.6 the same closure would bind both formals and compile.
//
// Expectation (oracle at 3.7): undefined_identifier on the body `_`.

void main() {
  final callback = (int _, int _) => _;
  print(callback(1, 2));
}
