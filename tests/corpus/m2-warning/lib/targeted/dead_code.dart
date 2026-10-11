// Targets dead_code: statements after a terminating return are unreachable
// (dead_code_verifier's dead-interval model).
void f() {
  return;
  1;
}
