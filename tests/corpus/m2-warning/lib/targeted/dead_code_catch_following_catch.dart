// Targets dead_code_catch_following_catch: a catch-all clause followed by a
// further catch clause makes the tail dead (_CatchClausesVerifier).
void f() {
  try {
    throw 'x';
  } catch (e) {
    print(e);
  } on Exception {
    print(1);
  }
}
