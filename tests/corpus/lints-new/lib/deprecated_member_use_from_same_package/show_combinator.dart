// deprecated_member_use_from_same_package — directive faces (#1160).
// Source: upstream Dart SDK source,
// pkg/linter/test/rules/deprecated_member_use_from_same_package_test.dart.
// POSITIVE upstream test_deprecatedClass_usedInShowCombinator: the shown
// deprecated name reports (upstream expects unused_import + the lint at
// the `C` inside `show C`).
// Expected rows: 1 — DEPRECATED_MEMBER_USE_FROM_SAME_PACKAGE keyed
// show_combinator.dart at the `C` in the combinator.
import 'deprecated_class.dart' show C;
