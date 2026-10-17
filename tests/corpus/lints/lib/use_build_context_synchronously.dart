// use_build_context_synchronously — official rule faces
// (pkg/linter use_build_context_synchronously.dart:939-1140; test/rules/
// use_build_context_synchronously_test.dart:2234-2241,1324-1335, upstream
// Dart SDK linter test data): a BuildContext-typed use evaluated behind an
// await crossing reports at the use ("Don't use 'BuildContext's across
// async gaps."), upgraded to "... across async gaps, guarded by an
// unrelated 'mounted' check." when a mounted check stood in guard position
// but resolved to the wrong association. A valid mounted check in guard
// position keeps the use silent.
//
// FLUTTER-DEPENDENCE POSTURE: the official identification is element
// identity with Flutter's BuildContext (analyzer
// src/utilities/extensions/flutter.dart:383-390, `isBuildContext` ->
// `_isExactly` against package:flutter/src/widgets/framework.dart), and
// every upstream test imports package:flutter/widgets.dart. This corpus
// package is plain Dart (pubspec.yaml has no flutter dep), so the oracle
// can never fire here. The dartq face resolves BuildContext-typed values
// by declaration spelling but silences a whole unit that declares a
// BuildContext-named type (the official element-identity posture: a
// same-file shadow must not be mis-judged by name). The stub class below
// therefore holds BOTH sides silent — 0 expected rows on every shape —
// and the differential value is pinning that mutual silence against a
// by-name over-fire regression. The rule stays unnamed in the corpus root
// options for the same reason (naming it would only judge a vacuous
// 0-vs-0 face); swap this stub for package:flutter/widgets.dart and add
// the options name in the same edit when the corpus gains flutter
// resolution.
//
// Shapes (both sides silent; upstream test anchors cited for the swap-in):
class BuildContext {
  bool get mounted => true;
}

// test_await_beforeReferenceToContext: the use sits behind an await
// crossing.
Future<void> f(BuildContext context) async {
  await Future<void>.value();
  print(context);
}

// test_ifStatement_referenceInThen_mountedInCondition: a valid mounted
// check in the condition guards the then branch.
Future<void> g(BuildContext context) async {
  await Future<void>.value();
  if (context.mounted) {
    print(context);
  }
}
