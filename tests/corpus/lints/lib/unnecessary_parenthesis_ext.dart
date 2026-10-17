// The import target for unnecessary_parenthesis.dart's imported-
// extension face (pkg/linter unnecessary_parenthesis.dart:120-124):
// the accessor resolves through the import to an extension whose `on`
// type is written nullable, so the parentheses stay and the one-token
// face keeps silent on both sides.
// Expected rows: 0.
extension NullableStringExt on String? {
  bool get g => false;
}
