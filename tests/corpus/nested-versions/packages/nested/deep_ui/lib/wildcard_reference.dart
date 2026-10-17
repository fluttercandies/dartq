// Version-gated error at language version 3.7, class-method form of the
// wildcard flip: with non-binding wildcards (3.7) the body's `_` has no
// declaration in scope; at 3.6 both formals would bind and the reference
// would be legal.
//
// Expectation (oracle at 3.7): undefined_identifier on the body `_`.

class Grid {
  int cell(int _, int _) => _;
}
