// deprecated_member_use_from_same_package — directive faces (#1160).
// Source: upstream Dart SDK source,
// pkg/linter/test/rules/deprecated_member_use_from_same_package_test.dart.
// POSITIVE upstream test_deprecatedLibrary_import: importing a library
// annotated @deprecated reports at the whole directive (upstream also
// expects an unused_import error — a WARNING, outside the lint filter).
// Expected rows: 1 — DEPRECATED_MEMBER_USE_FROM_SAME_PACKAGE keyed
// import_deprecated.dart at the import directive.
import 'deprecated_lib.dart';
