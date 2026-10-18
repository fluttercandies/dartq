// unnecessary_lambdas — official rule faces
// (pkg/linter unnecessary_lambdas.dart:97-232): a lambda whose whole body
// is one invocation of a callable every argument of which is the lambda's
// own parameter reports at the lambda (a tear-off will do). The dartq
// finality face resolves bare receivers/callees over the same-file
// substrate: a non-`final` variable, a parameter, or a getter receiver
// bails; an unresolvable name proceeds.
//
// SEMANTIC-FACE marker: this rule is the #1095 semantic-bridge wave's
// first consumption row. It currently fires over the syntax face; once it
// joins the semantic activation list, these rows appear only when the
// semantic face mounts — the fixture documents the same trigger shape for
// either posture and the gate compares both sides under whatever face is
// live.
// Expected rows: 2 — UNNECESSARY_LAMBDAS keyed unnecessary_lambdas.dart,
// message "Closure should be a tearoff.":
//   the `(e) => print(e)` lambda in `forEach`
//   the `(e) => print(e)` lambda in `h2`'s initializer — the oracle-of-
//   record's arrow-in-initializer face (3.13.1 reports there while the
//   block-body initializer stays silent; #2095)
void f() {
  [].forEach((e) => print(e));

  final h2 = (e) => print(e);
  h2(1);

  // GOOD: a non-`final` receiver demotes the tear-off verdict, and an
  // async lambda never reduces.
  var l = <int>[];
  [].where((e) => l.contains(e));
  [].forEach((e) async => print(e));
}
