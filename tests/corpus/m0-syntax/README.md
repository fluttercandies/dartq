# m0-syntax corpus

Syntax-broken corpus for the M0 differential gate (`dartq` vs
`dart analyze`, Jaccard over `(code, file)` entry sets). Read-only
assembled snapshot; see `docs/facts/corpus-baseline.md`. Language version:
3.13 (`pubspec.yaml`, sdk ^3.13.0) — several fixture notes below hinge on
that version.

## Blindspot fixtures (`lib/targeted/blindspot_*.dart`)

Added in commit 018552d: 23 fixtures exercising the syntax codes with zero
recorded gate exposure. Honest count at language 3.13: **58-59 of the 63
blind-spot codes are exercisable** — not 63. The remainder are documented
gaps by construction:

- `unexpected_dollar_in_string` (TSV:282) — never fires: the analyzer's
  error-token translation remaps it to `missing_identifier` before
  reporting (`translate_error_token.dart:105-106`); dartq mirrors the
  remap, so the face is covered via `missing_identifier`
  (`blindspot_string_dollar.dart`).
- `missing_quote` (TSV:219) — never fires: no emit site anywhere in the
  SDK (generated diagnostic definition only) and none in dartq; the
  scanner-tier quote face is covered via `unterminated_string_literal`
  (TSV:288) in `blindspot_library_directives.dart`.
- `expected_representation_type` (TSV:103) — version-gated: fires only
  with primary constructors disabled, and the feature shipped in 3.13
  (`blindspot_extension_types.dart`).
- `part_of_name` (TSV:252) — version-gated: enhanced-parts off by default
  at 3.13, so the legacy shape is legal and emits nothing; the fixture
  pins that legal-legacy face (`blindspot_part_of.dart`). Counting it as
  exercisable gives 59, not counting it gives 58.
- `invalid_use_of_identifier_augmented` (TSV:187) — oracle-side
  uncertainty: the default oracle configuration may treat `augmented` as a
  plain identifier (`blindspot_experiments.dart`).

Pre-review corrections applied (fixtures are the authority):

- `blindspot_string_dollar.dart`: retargeted — the face is covered via the
  `missing_identifier` remap; `unexpected_dollar_in_string` reclassified
  into the never-fires set.
- `blindspot_extension_types.dart`: `extension type X {` (no parens) hits
  `missing_primary_constructor` (TSV:217), not `expected_representation_type`;
  added `extension type X() {}` targeting `expected_representation_field`
  (TSV:102, empty parens = declaration present, field missing).
- `blindspot_experiments.dart`: primary constructors shipped in 3.13, so
  `class PrimaryPoint(final int x) {}` is valid and emits nothing;
  documented the variance variant fork — the oracle adds
  `experiment_not_enabled_off_by_default` (TSV:108) alongside
  `multiple_variance_modifiers` (TSV:237); dartq currently emits only the
  latter (real gap, documented not hidden).
- `blindspot_part_of.dart`: zero-exposure caveat added (enhanced-parts off
  by default at 3.13); fixture kept to pin the legal-legacy face.
- `blindspot_library_directives.dart`: `missing_quote` reclassified as
  never-fires; added `import 'a.dart;` targeting
  `unterminated_string_literal` (TSV:288); the configuration-URI shape
  relabeled as plausibly `non_string_literal_as_uri` (TSV:248).

### Index

| Fixture | Family / codes |
| --- | --- |
| `blindspot_accessors.dart` | accessors: `missing_get`, `static_getter_without_body` |
| `blindspot_annotation.dart` | `annotation_with_type_arguments` (+ uninstantiated variant) |
| `blindspot_arguments.dart` | `positional_after_named_argument` |
| `blindspot_async_modifier.dart` | async modifiers: `invalid_sync`, `invalid_star_after_async` |
| `blindspot_doc_comment.dart` | `invalid_comment_reference` |
| `blindspot_encoding.dart` | `encoding` — the file is deliberately non-UTF-8 |
| `blindspot_enum.dart` | `empty_enum_body` |
| `blindspot_experiments.dart` | experiment-gated family: `experiment_not_enabled`/`_off_by_default`, `extension_augmentation_has_on_clause`, `extraneous_modifier_in_primary_constructor`, `multiple_variance_modifiers`, `invalid_use_of_identifier_augmented` |
| `blindspot_extension_types.dart` | extension types: `missing_primary_constructor`, `expected_representation_field`, `representation_field_trailing_comma`, `extraneous_modifier_in_extension_type` |
| `blindspot_external_members.dart` | external members: `external_getter_with_body` et al. |
| `blindspot_factory_constructors.dart` | factory/initializer family |
| `blindspot_function_types.dart` | `named_function_type`, `missing_function_keyword`, `invalid_generic_function_type` |
| `blindspot_library_directive.dart` | `missing_name_in_library_directive` |
| `blindspot_library_directives.dart` | library/URI family: `non_identifier_library_name`, `non_string_literal_as_uri`, `invalid_literal_in_configuration`, `unterminated_string_literal` |
| `blindspot_modifiers.dart` | member-modifier family |
| `blindspot_native.dart` | native family (TSV:242-244) — SDK-gated; oracle may suppress, codes stay documented gaps |
| `blindspot_parameter_groups.dart` | `mixed_parameter_groups`, `multiple_named_parameter_groups`, `multiple_positional_parameter_groups` |
| `blindspot_part_of.dart` | `part_of_name` — zero-exposure at 3.13, pins the legal-legacy face |
| `blindspot_part_of_missing_name.dart` | `missing_name_in_part_of_directive` |
| `blindspot_string_dollar.dart` | dollar-in-string face via `missing_identifier`; `unexpected_dollar_in_string` never fires |
| `blindspot_switch_statement.dart` | `expected_case_or_default` |
| `blindspot_type_literals.dart` | type-literal family: `expected_list_or_map_literal`, `expected_named_type` |
| `blindspot_var_declarations.dart` | `var` declaration family: `var_class`, `var_enum`, et al. |

Per-code anchors (TSV row numbers, fasta/analyzer emit sites, and
oracle-uncertainty notes) live in each fixture's header comment — the
fixtures are the authority; this README only indexes them.
