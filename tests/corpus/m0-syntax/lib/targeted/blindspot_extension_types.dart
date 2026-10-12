// Targets the extension-type family.
// Targets missing_primary_constructor (TSV:217): the shape below omits the
// representation declaration entirely; fasta reports missingPrimaryConstructor
// when an extension type has no primary constructor part
// (pkg/_fe_analyzer_shared/lib/src/parser/parser_impl.dart:3876).
// Pre-review this shape targeted expected_representation_type, but that
// code is unreachable at language 3.13 — see the never-fires note below.
extension type MissingRepresentation {
  int get x => 1;
}

// Targets expected_representation_field (TSV:102): empty parens — the
// representation declaration is present but names no field. Emitted by the
// primary-constructors-enabled branch (pkg/front_end/lib/src/source/
// outline_builder.dart:1954, formals.isEmpty).
extension type EmptyRepresentation() {}

// Never-fires face at 3.13 (TSV:103): expected_representation_type fires
// only when primary constructors are DISABLED and a representation formal
// has no type (outline_builder.dart:1886, e.g. 'extension type X(x)').
// The feature shipped in 3.13 (experimental_features.yaml:200, enabledIn
// '3.13.0'), so at the corpus language version the disabled branch is
// unreachable. Version-gated documented gap, like the native family.

// Dead face at 3.13 (TSV:263, probed 2026-10-11): the fasta message key
// representationFieldTrailingComma exists (pkg/front_end/lib/src/codes/
// diagnostic.g.dart), but the analyzer has no emitter for it —
// `extension type E(int x,) {}` and this shape are both silent in
// `dart analyze` output at the pinned oracle. Kept as a zero-row gate face;
// no dartq emitter (emitter-wave W1 probe verdict).
extension type TrailingComma(int x,) {}

// Targets extraneous_modifier_in_extension_type (TSV:128) — fasta emit
// site: pkg/_fe_analyzer_shared/lib/src/parser/modifier_context.dart:233
// (covariant on an extension type member).
extension type CovariantMember(int v) {
  covariant void m() {}
}
