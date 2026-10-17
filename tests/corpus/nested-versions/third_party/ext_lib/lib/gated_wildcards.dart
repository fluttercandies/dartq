// Version-gated error at language version 3.0: wildcard formals are a
// 3.7 feature, so both `_` parameters bind here and the second
// collides with the first.
//
// Expectation (oracle at 3.0): duplicate_definition on the second `_`
// parameter.

String tag(int _, int _) => 'tag';

void main() {
  print(tag(1, 2));
}
