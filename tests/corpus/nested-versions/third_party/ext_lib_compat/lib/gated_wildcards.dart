// Version-gated error at language version 2.19: wildcard formals are a
// 3.7 feature, so both `_` parameters bind here and collide.
//
// Expectation (oracle at 2.19): duplicate_definition on the second `_`
// parameter.

int choose(int _, int _) => 0;

void main() {
  print(choose(1, 2));
}
