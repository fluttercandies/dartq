// deprecated_member_use_from_same_package — directive faces (#1160).
// Source: upstream Dart SDK source,
// pkg/linter/test/rules/deprecated_member_use_from_same_package_test.dart.
// NEGATIVE upstream test_deprecatedClass_usedInHideCombinator: hiding the
// deprecated name reports nothing (upstream expects only an unused_import
// error, outside the lint filter).
import 'deprecated_class.dart' hide C;
