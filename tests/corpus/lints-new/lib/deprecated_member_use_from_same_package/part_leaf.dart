// deprecated_member_use_from_same_package — directive faces (#1160).
// Source: upstream Dart SDK source,
// pkg/linter/test/rules/deprecated_member_use_from_same_package_test.dart.
// POSITIVE upstream test_deprecatedLibrary_export_inSubpart: exporting the
// deprecated library from inside a depth-2 sub-part reports at the whole
// directive. Expected rows: 1 — DEPRECATED_MEMBER_USE_FROM_SAME_PACKAGE
// keyed part_leaf.dart at the export directive.
part of 'part_mid.dart';

export 'deprecated_lib.dart';
