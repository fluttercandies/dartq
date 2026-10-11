// Boundary wildcards: a local named exactly `_` is a wildcard and stays
// silent with the wildcard-variables feature enabled (this corpus's ^3.13
// feature set) - only the exact `_` name is exempt; other all-underscore
// names like `__` still warn (unused_local_variable_test.dart:172
// test_localVariable_underscores).
void f() {
  var _ = 1;
  int _ = 2;
  final _ = 3;
  var __ = 1; // warns unused_local_variable once (unused_local_variable_test.dart:172 test_localVariable_underscores)
}
