// Targets the extension-type family.
// Targets expected_representation_type (syntax-codes.tsv:103) — fasta
// message key expectedRepresentationType ("Expected a representation
// type.", pkg/front_end/lib/src/codes/diagnostic.g.dart): the
// representation declaration is missing.
extension type ExpectedRepresentationType {
  int get x => 1;
}

// Targets representation_field_trailing_comma (TSV:263) — fasta message
// key representationFieldTrailingComma (same generated file): the
// representation field can't have a trailing comma.
extension type TrailingComma(int x,) {}

// Targets extraneous_modifier_in_extension_type (TSV:128) — fasta emit
// site: pkg/_fe_analyzer_shared/lib/src/parser/modifier_context.dart:233
// (covariant on an extension type member).
extension type CovariantMember(int v) {
  covariant void m() {}
}
