// no_wildcard_variable_uses — official rule faces (pkg/linter
// no_wildcard_variable_uses.dart): a use of an all-underscores local
// variable or formal parameter, with the element test excluding fields,
// getters, and everything else that is not a local or parameter.
// The rule registers only when the wildcard-variables feature is OFF
// (the visitor registration bails otherwise), so this library pins
// `// @dart = 3.4` exactly like the official test data — the
// 3.13-pinned corpus default leaves the rule silent on both sides.
// Expected rows: 2 — NO_WILDCARD_VARIABLE_USES keyed
// no_wildcard_variable_uses.dart:
//   `print(_)` in localUser   -> use of the `_` local
//   `print(__)` in paramUser  -> use of the `__` parameter
// The class face is negative by design: `_` there is a field, and the
// use resolves to the field element, which the rule never reports.
// @dart = 3.4

void localUser() {
  var _ = 1;
  print(_);
}

void paramUser(int __) {
  print(__);
}

class FieldHolder {
  int _ = 0;

  void m() {
    print(_);
  }
}
