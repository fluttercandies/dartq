# Manual conformance cases (hand-written)

Cases under `diagnostic/<code>/<n>.case`, id `manual/diagnostic/<code>/<n>`.
Every trigger was written from the upstream Dart SDK analyzer `messages.yaml`
entry for the code **and** the analyzer implementation that emits it (cited per
row below); nothing is guessed from the message text alone. `/1` is always the
minimal positive trigger; `/2` (and `/3` where present) are negative or
boundary cases so the oracle run covers both sides of the check.

Cited implementation files are upstream analyzer sources:
`pkg/analyzer/lib/src/generated/ffi_verifier.dart`,
`pkg/analyzer/lib/src/analysis_options/analysis_options_parse_model.dart`,
`pkg/analyzer/lib/src/error/best_practices_verifier.dart`,
`pkg/analyzer/lib/src/error/unicode_text_verifier.dart`,
`pkg/analyzer/lib/src/dart/analysis/library_analyzer.dart`,
`pkg/analyzer/lib/src/dart/type_instantiation_target.dart`.

## FFI family (49 codes, 99 cases)

| Code | Cases | Trigger source |
| --- | --- | --- |
| abi_specific_integer_invalid | 2 | messages.yaml#abiSpecificIntegerInvalid; ffi_verifier `_validateAbiSpecificIntegerAnnotation` (members != 1 const ctor / type params) |
| abi_specific_integer_mapping_extra | 2 | `_validateAbiSpecificIntegerMappingAnnotation` (>1 mapping annotation) |
| abi_specific_integer_mapping_missing | 2 | same (no mapping annotation) |
| abi_specific_integer_mapping_unsupported | 2 | same (`_primitiveIntegerNativeTypesFixedSize` check); /1 Array value per official doc example, /2 IntPtr (integer but not fixed-size) |
| address_position | 2 | `_validateAddressPosition` (`.address` outside a leaf `@Native(isLeaf: true)` call); official doc example shapes |
| address_receiver | 2 | `_validateAddressReceiver` (primitive receiver: local int vs struct field) |
| annotation_on_pointer_field | 2 | `_validateNoAnnotations` via `_validateFieldsInCompound` (ffi annotation on Pointer field) |
| argument_must_be_a_constant | 2 | `_validateFromFunction` -> `_isConst` (non-const exceptional return vs const literal) |
| argument_must_be_native | 2 | `_validateNativeAddressOf` (unannotated target vs `@Native`-annotated) |
| compound_implements_finalizable | 2 | visitClassDeclaration (Struct subclass implementing Finalizable) |
| creation_of_struct_or_union | 2 | visitInstanceCreationExpression (generative ctor call vs calloc.allocate via package:ffi mock) |
| empty_struct | 2 | `isEmptyStruct` element check; official doc example |
| extra_annotation_on_struct_field | 2 | `_validateAnnotations` (second ffi annotation) |
| extra_size_annotation_carray | 2 | `_validateSizeOfAnnotation` (>1 `@Array`) |
| ffi_native_invalid_duplicate_default_asset | 2 | visitLibraryDirective (two `@DefaultAsset` on the library directive); official doc example ordering |
| ffi_native_invalid_multiple_annotations | 2 | `_checkFfiNative` (two `@Native` annotations) |
| ffi_native_must_be_external | 2 | `_checkFfiNative` (`!isExternal`) |
| ffi_native_only_classes_extending_nativefieldwrapperclass1_can_be_pointer | 2 | `_checkFfiNativeFunction` instance-method receiver check (plain class vs `extends NativeFieldWrapperClass1`); param count kept matching so only this code fires |
| ffi_native_unexpected_number_of_parameters | 2 | `_checkFfiNativeFunction` top-level param-count mismatch |
| ffi_native_unexpected_number_of_parameters_with_receiver | 2 | instance-method count mismatch (expected params+1) |
| field_must_be_external_in_struct | 2 | `_validateFieldsInStruct`/`_validateFieldsInCompound` non-external field; official doc shape (`@Int16() int a;`) keeps exactly one diagnostic |
| generic_struct_subclass | 2 | visitClassDeclaration type-parameter check |
| invalid_exception_value | 2 | `_validateFromFunction` (exceptional return with void/Handle/Pointer return type) |
| invalid_field_type_in_struct | 2 | String field vs Pointer field; official doc shapes |
| leaf_call_must_not_return_handle | 2 | `_validateFfiLeafCallUsesNoHandles` via `asFunction(isLeaf: true)`; official doc shape |
| leaf_call_must_not_take_handle | 2 | same, Handle parameter |
| mismatched_annotation_on_struct_field | 2 | `_validateAnnotations` (annotation type != field type) |
| missing_annotation_on_struct_field | 2 | `_validateAnnotations` (`!requiredFound`) |
| missing_exception_value | 2 | `_validateFromFunction` (1 arg, non-void return) |
| missing_field_type_in_struct | 2 | `fieldType == null` (`external var`) |
| missing_size_annotation_carray | 2 | `_validateSizeOfAnnotation` (no `@Array`) |
| must_be_a_native_function_type | 2 | `_validateFromFunction` with `T extends Function` type argument; official doc shape |
| must_be_a_subtype | 2 | `_validateFromFunction` compatible-function-types check; official doc shapes |
| must_return_void | 2 | `_validateNativeCallable` (`NativeCallable.listener` non-void return) |
| native_field_invalid_type | 2 | field annotated with a function type (`_checkFfiNative` else-branch) |
| native_field_missing_type | 2 | `_checkFfiNativeField` with `@Native()` (`_canonicalFfiTypeForDartType(int) == null`) |
| native_field_not_static | 2 | `_checkFfiNativeField` non-static class field |
| native_function_missing_type | 2 | `_checkFfiNativeFunction` with omitted type argument and non-void return |
| negative_variable_dimension | 2 | `_validateSizeOfAnnotation` (`Array.variableWithVariableDimension(-1)`; /2 boundary 0 is legal) |
| non_constant_type_argument | 2 | `_validateSizeOf` with type-parameter argument |
| non_native_function_type_argument_to_pointer | 2 | `_validateAsFunction` with Dart types in the C signature; official doc shapes |
| non_positive_array_dimension | 2 | `@Array(-8)` vs `@Array(8)` |
| non_sized_type_argument | 2 | `_isSized` (Array<Void> vs Array<Uint8>) |
| packed_annotation | 2 | `_validatePackedAnnotation` (two `@Packed`) |
| packed_annotation_alignment | 2 | `_validatePackedAnnotation` (alignment 3 vs 4) |
| size_annotation_dimensions | 2 | annotation dimension count != type dimension count |
| subtype_of_struct_class | 3 | `subtypeOfStructClassInExtends` / `InImplements` / `InWith` (all three sharedName variants; see risks) |
| use_of_native_extension | 2 | library_analyzer `_reportImportDirectiveErrors` (`dart-ext:` import / export) |
| variable_length_array_not_last | 2 | variable `@Array` not in last-field position vs last |

## Warning-level codes (17 codes, 31 cases)

| Code | Cases | Trigger source |
| --- | --- | --- |
| deprecated_subtype_of_function | 2 | best_practices_verifier; `extends Function` / `implements Function` (both variants share the registry code name) |
| text_direction_code_point_in_comment | 2 | unicode_text_verifier (U+202A..U+202E, U+2066..U+2069 in comments); literal U+202E embedded in the fixture bytes |
| text_direction_code_point_in_literal | 2 | same verifier, string-literal occurrence; /2 negative keeps the code point in a comment only |
| invalid_use_of_visible_for_template_member | 2 | best_practices_verifier `_checkForOtherInvalidAccess`; `@visibleForTemplate` (package:angular_meta mock) used from another library; /2 negative is a same-library use (`_inCurrentLibrary` exemption) |
| undefined_lint | 2 | parse_model `_parseLocalRules` (unregistered rule name vs real rule) |
| duplicate_rule | 2 | same rule enabled twice |
| deprecated_lint | 2 | `avoid_null_checks_in_equality_operators` (RuleState.deprecated since 3.11, no replacement) vs a stable rule |
| removed_lint | 2 | `super_goes_last` (RemovedAnalysisRule since Dart 3.0) vs a stable rule |
| incompatible_lint | 2 | `always_use_package_imports` + `prefer_relative_imports` (declared incompatibleRules) vs one alone |
| unsupported_option | 2 | lint rule value `sometimes` (not in true/false/ignore/...) vs `true` |
| unrecognized_error_code | 2 | `analyzer.errors` key that is not a diagnostic code vs `dead_code` |
| invalid_section_format | 2 | `analyzer:` section holding a list instead of a map vs a map |
| analysis_options_deprecated_plugins | 2 | legacy `analyzer: plugins:` key present vs absent |
| include_file_not_found | 2 | `include:` of a missing file vs an existing one (second file section) |
| recursive_include_file | 1 | `analysis_options.yaml` includes itself |
| included_file_warning | 1 | included file carries an undefined-lint warning; walker wraps it at the include site (see risks) |
| plugins_in_inner_options | 1 | nested `lib/analysis_options.yaml` (not at the context root) with a `plugins:` section (see risks) |

## Addendum

| Code | Cases | Trigger source |
| --- | --- | --- |
| wrong_number_of_type_arguments_function | 2 | type_instantiation_target `InvocationTargetFunctionTypedExpression`: `g<int><int>(0)` on a generic function-typed parameter (outer reference has 0 type params, 1 given) vs the correct single instantiation |

## Skipped codes and why

- `analysis_option_deprecated`: verified to have **no emission site** in the
  pinned analyzer (referenced only in the generated diagnostic tables), so the
  oracle cannot produce expected output for it in this SDK generation.
- `missing_dependency`: **not emitted by `dart analyze`**. The code is
  produced only by `MissingDependencyValidator` (upstream
  `pkg/analyzer/lib/src/pubspec/validators/missing_dependency_validator.dart`),
  which the analysis server's pubspec fix processor invokes with the set of
  dependencies used by the sources (`analysis_server/.../pubspec_fix_processor.dart`);
  the `dart analyze` CLI path only runs the seven static validators of
  `validatePubspec`, so the oracle can never produce expected output for it.
- `deprecated_lint_with_replacement`: needs a deprecated rule carrying
  `replacedBy`; the pinned linter registry has three deprecated rules
  (avoid_null_checks_in_equality_operators, prefer_final_parameters,
  use_if_null_to_convert_nulls_to_bools) and none carries a replacement.
- `replaced_lint`: needs a removed rule carrying `replacedBy`; every
  RemovedAnalysisRule in the pinned registry (super_goes_last,
  prefer_equal_for_default_values, prefer_bool_in_asserts, invariant_booleans,
  ...) has no replacement.
- Pubspec/publish face (PubspecWarningCode + manifest/custom-lint warnings):
  asset_directory_does_not_exist, asset_does_not_exist, asset_field_not_list,
  asset_missing_path, asset_not_string, asset_not_string_or_map,
  asset_path_not_string, camera_permissions_incompatible,
  dependencies_field_not_map, deprecated_field, flutter_field_not_map,
  invalid_dependency, invalid_platforms_field, missing_dependency, missing_name,
  multiple_plugins, name_not_string, no_touchscreen_feature,
  non_resizable_activity, path_does_not_exist, path_not_posix,
  path_pubspec_does_not_exist, permission_implies_unsupported_hardware,
  platform_value_disallowed, setting_orientation_on_activity, unknown_platform,
  unnecessary_dev_dependency, unsupported_chrome_os_feature,
  unsupported_chrome_os_hardware, workspace_field_not_list,
  workspace_value_not_string, workspace_value_not_subdirectory.
  These fire from pubspec.yaml validation (official doc examples carry
  `%uri="pubspec.yaml"`), not from analyzing a Dart file. The harness
  materializes its own pubspec for the case package and no extracted case ever
  declares a pubspec.yaml file section, so a hand case would need a harness
  capability decision first. Judged hand-untestable in this batch.
  **Resolved by lane #2721**: the runner now loads `conformance/manual/` and
  the materializer honors case-supplied `pubspec.yaml` /
  `analysis_options.yaml` sections, so all of these became writable — see
  "Pubspec and manifest family (31 codes, 62 cases)" below. Only
  `missing_dependency` stayed skipped (reason in Skipped codes).

## Pubspec and manifest family (31 codes, 62 cases, lane #2721)

Each case's `pubspec.yaml` (and for the manifest rows the
`android/AndroidManifest.xml` plus the `optional-checks` gate) is carried by a
file section; the materializer uses it verbatim for the package root.
`/1` is the minimal positive trigger, `/2` the negative boundary. Emission
sites were pinned from the pinned SDK's `dart analyze` CLI path: pubspec
diagnostics come from `pkg/analyzer/lib/src/pubspec/validators/*.dart`, the
manifest codes from `pkg/analyzer/lib/src/manifest/manifest_validator.dart`,
and `multiple_plugins` from the analysis-options parser
(`pkg/analyzer/lib/src/analysis_options/analysis_options_parse_model.dart`).
A throwaway probe against the oracle SDK confirmed before writing that the CLI
does report pubspec diagnostics and that the manifest gate key is
`analyzer: optional-checks: chrome-os-manifest-checks: true` (placing the key
directly under `analyzer:` yields `unsupported_option` instead).

| Code | Cases | Trigger source |
| --- | --- | --- |
| asset_directory_does_not_exist | 2 | flutter_validator `_validateAssetPath` (`assets` entry ending `/` with no folder) vs existing directory |
| asset_does_not_exist | 2 | same (file entry without folder) vs existing file |
| asset_field_not_list | 2 | flutter_validator (`assets` scalar vs empty list) |
| asset_missing_path | 2 | flutter_validator (asset map without `path`) vs map with existing path |
| asset_not_string | 2 | flutter_validator (`path` non-string scalar) vs string path |
| asset_not_string_or_map | 2 | flutter_validator (asset entry non-string scalar) vs string asset |
| asset_path_not_string | 2 | flutter_validator (`path` a list) vs scalar path |
| camera_permissions_incompatible | 2 | manifest `_validatePermissions` (CAMERA without camera features vs both declared `required="false"`) |
| dependencies_field_not_map | 2 | dependency_validator `getDeclaredDependencies` (scalar vs empty map) |
| deprecated_field | 2 | field_validator `_deprecatedFields` (`author`) vs a non-deprecated key |
| flutter_field_not_map | 2 | flutter_validator (non-empty non-map `flutter` vs empty section, which is explicitly allowed) |
| invalid_dependency | 2 | dependency_validator `validatePathEntries` (publishable = `version` present and `publish_to` not `none`; git dep vs no version) |
| invalid_platforms_field | 2 | platforms_validator (scalar vs map with null values) |
| missing_name | 2 | name_validator (no `name` field, reported at offset 0 vs name present) |
| multiple_plugins | 2 | parse_model `_ParsedLegacyPluginsData` (`plugins: [aaa, bbb]` — any second *distinct* name conflicts; `[aaa, aaa]` does not) |
| name_not_string | 2 | name_validator (non-string scalar vs string) |
| no_touchscreen_feature | 2 | manifest `_validateTouchScreenFeature` (no feature vs `required="false"`) |
| non_resizable_activity | 2 | manifest `_validateActivity` (`resizeableActivity="false"` vs `"true"`) |
| path_does_not_exist | 2 | dependency_validator (path dep to missing dir vs existing package dir; the screenshot validator shares the code) |
| path_not_posix | 2 | dependency_validator (`path` containing `\` vs a posix path) |
| path_pubspec_does_not_exist | 2 | dependency_validator (existing dir without pubspec.yaml vs with one) |
| permission_implies_unsupported_hardware | 2 | manifest `_validatePermissions` (`CALL_PHONE` implies telephony vs `INTERNET`) |
| platform_value_disallowed | 2 | platforms_validator (non-null platform value vs null) |
| setting_orientation_on_activity | 2 | manifest `_validateActivity` (`screenOrientation="portrait"` vs `"unspecified"`) |
| unknown_platform | 2 | platforms_validator (`brower` vs `linux`) |
| unnecessary_dev_dependency | 2 | dependency_validator (dev dep also in `dependencies` vs alone) |
| unsupported_chrome_os_feature | 2 | manifest `_validateFeatures` (unsupported feature with `required="true"` vs `"false"`) |
| unsupported_chrome_os_hardware | 2 | manifest `_validateFeatures` (unsupported feature without `required` vs `"false"`) |
| workspace_field_not_list | 2 | workspace_validator (scalar vs list of an existing directory) |
| workspace_value_not_string | 2 | workspace_validator (non-string entry vs string entry) |
| workspace_value_not_subdirectory | 2 | workspace_validator (`../escape` vs an entry inside the package root) |

## Flow families (3 codes, 16 cases, lane #2727)

Hand-written from the 2026-10-29 flow-face coverage audit's manual-case
candidates (audit families F3, F4, F11, F13, F14, F16, F19). Shapes were
pinned against upstream sources before writing:
`pkg/_fe_analyzer_shared/lib/src/flow_analysis/flow_analysis.dart` (the
flow engine: suspension, member reachability, property-slot sweeps, loop
exits), `pkg/analyzer/lib/src/dart/resolver/assignment_expression_resolver.dart`
(staged if-null write types) and `pkg/linter/lib/src/rules/use_build_context_synchronously.dart`
(async-gap verdicts). All 16 cases were captured in one oracle run against
the pinned 3.13.7 SDK (`oracle --filter manual/`: 210 manual cases, 16
analyzed, 0 crashes, 21 rows for this batch); the polarities below are that
run's output, not the audit's guesses. Silence cases (`-` in
`expected.tsv`) are valid parity pins: any dartq extra row breaks 1:1.

| Code | Cases | Shapes |
| --- | --- | --- |
| dead_code | 6 | /1 `??` with a guaranteed-non-null LHS — oracle deads the right operand (`dead_code` at the `??` plus `dead_null_aware_expression` on the RHS, both faces in one case); /2 nullable-LHS control (silence); /3 default-first switch (dead rows on the `default` keyword, the later case head and its body, plus structural error `switch_has_case_after_default_case`); /4 second default group (only the second `default` keyword + body die; error `switch_has_multiple_default_cases`); /5 `case 1: default:` stacked shared body (the shared case head stays live; `default` keyword, later case head and body die; `switch_has_case_after_default_case`); /6 statements after break-free `while (true)` / `for (;;)` (both tails die) |
| unchecked_use_of_nullable_value | 7 | /1 `sync*` + `??=` + member read after `yield` (silence — the generator body's own suspension demotes nothing); /2 `async*` twin (silence); /3 type-test guard across `yield` in a generator + plain-function `??=` control (silence); /4 await-demotion twins: `await` in a plain async method demotes nothing (that read is clean) while inside a local async closure BOTH reads of the captured written variable report (divergence — see risks); /5 staged `??=` with a resolved nullable call RHS (the read after the write reports); /6 staged `??=` with resolved non-null constructor/call RHS (silence); /7 property-slot sweep: the guarded `_p` read before a receiver reassignment is clean, the read after it reports |
| use_build_context_synchronously | 3 | /1 `await` inside a try body leaves the gap open into `finally` (the context use there reports); /2 a not-mounted exit guard does not survive a second `await` (the use after it reports); /3 guarded-use control (silence) |

### Flow batch risks and divergences

1. `unchecked_use_of_nullable_value/4`: the audit's pin family expects the
   closure's pre-await read to carry the `??=` promotion and only the
   post-await read to report; the pinned oracle flags both reads — inside
   the closure the captured variable's `??=` write holds no promotion for
   the first read at all. The case stays as written (oracle is the truth;
   the two-row output is the parity target). The plain-async half is
   confirmed: the oracle emits nothing for the method-body read after
   `await g()`.
2. `dead_code/3`-`/5`: the member-order shapes are genuinely erroneous
   Dart, so the oracle pairs the flow dead rows with structural errors
   (`switch_has_case_after_default_case`, `switch_has_multiple_default_cases`);
   dartq must reproduce both faces to stay 1:1.
3. `dead_code/1`: the oracle emits `dead_code` at the `??` operator AND a
   `dead_null_aware_expression` row on the right operand — the same
   pairing the extracted dead-null-aware shapes show.
4. The generator silence shapes (`unchecked_use_of_nullable_value/1`-`/3`)
   pin the F13 false-positive side: any dartq suspension demotion in a
   generator body surfaces as an extra row and breaks parity. Severity
   note: the oracle reports `unchecked_use_of_nullable_value` as ERROR
   under the harness defaults, `dead_code` as WARNING, the F14 lint as
   INFO.
5. The F14 cases ride the `flutter`/`ffi`/`meta`/`angular_meta` mocks with
   the extracted-lints options section (`propagate-linter-exceptions` +
   the rule enabled), mirroring the extracted lints corpus.
6. Audit candidates left to a follow-up lane: the `null != null`
   known-constant fold (F3), the trailing-default silence control (F4),
   `is Never` reference folds (F20), `FutureOr`/record-bound promotion
   (F8), and the try/catch + labeled-case + closure-write property-slot
   sweeps (F11's remaining pins).

## Syntax codes (62 codes, 69 cases, lane #2744)

The parser-face recon's residual tail: the 234 uncovered syntax codes minus the
147 taken by the `parser` face extraction, the 10 by `parser-fe`, and the 15
already carried by `diagnostics/_misc` resolution cases. `/1` is the minimal
trigger; `/2` (7 codes) is the cheap silence/negative control. Two authoring
sources, per the recon:

- **Lifted (28 codes)**: the Dart input is byte-identical to the
  oracle-validated repro in the dartq parser gate pins / pin fixtures
  (`crates/dartq-parser/src/tests/**`); rust-side expectations were stripped —
  the case content is just the Dart source. `source:` cites the pin by name.
- **Authored (34 codes)**: written from the upstream `messages.yaml` entry
  (official example verbatim where one exists) plus the emission-site
  reasoning. Four codes the recon had assigned to the pin bucket
  (`external_factory_redirection`, `type_parameter_on_constructor`,
  `type_parameter_on_operator`, `var_as_type_name`) have no parser gate pin and
  were authored instead — for the latter two from their official CFE scripts.

Registry finding that shapes the polarity column: the fasta-based 3.13.7
oracle only emits a syntax code when a CFE template carries it via
`analyzerCode:` (`_fe_analyzer_shared/messages.yaml`). The old hand-written
parser's codes without a crosswalk entry are **registry-only** — they exist in
`diagnostic.g.dart` but have no emission site upstream or in dartq, and the
oracle run below proves the resolution/recovery face fires instead for their
classic shapes. Those cases stay as parity pins: any dartq extra row breaks
1:1, and the faces they pin are part of the differential surface.

All 69 cases were captured in one oracle run against the pinned 3.13.7 SDK
(`oracle --filter manual/`: 279 manual cases loaded, 279 analyzed, 0 crashes,
957 manual rows, 13.7s single chunk, peak RSS 0.06 GB). The polarity column is
that run's output, not the authoring-time guesses (which the run corrected in
four places, marked below).

| Code | Cases | Source / shape | Polarity (3.13.7 oracle run) |
| --- | --- | --- | --- |
| abstract_static_method | 1 | authored; ParserErrorCode#abstractStaticMethod shape `abstract static void m()` | registry-only: oracle emits `abstract_class_member` + `modifier_out_of_order` + `missing_function_body` |
| annotation_space_before_parenthesis | 2 | lifted pin `annotation_space_before_parenthesis` (`@metadata (x, y)`); /2 adjacent control | live (CFE annotationSpaceBeforeParenthesis): /1 fires, /2 shows only the undefined-name rows |
| annotation_with_type_arguments | 1 | authored; official messages.yaml#metadataTypeArguments example verbatim incl. `// @dart=2.12` | gate face wins: oracle emits `experiment_not_enabled` (generic-metadata off at 2.12); the target code itself stays unexercised in the CLI — case pins the experiment-gate face |
| covariant_constructor | 1 | authored; `covariant C()` | registry-only: oracle emits `covariant_member` |
| duplicate_deferred | 2 | lifted pin (deferred recovery trio, `deferred deferred as m`); /2 single-deferred control; companion `lib/a.dart` | live (CFE duplicateDeferred): /1 fires, /2 silent except `unused_import` |
| empty_enum_body | 1 | authored; `enum E {}` | registry-only: oracle emits `enum_without_constants` (not the `expected_body` family — authoring guess corrected) |
| expected_case_or_default | 1 | lifted group-A dead-code pin (`switch (x) { + }`) | registry-only: oracle emits `expected_token` + `undefined_identifier` |
| expected_list_or_map_literal | 1 | authored from the parser tree's `const <int>5` expression shape | registry-only: oracle emits `expected_token` |
| expected_named_type | 1 | authored; record type in the extends slot | LIVE: oracle emits the target code ("Expected a class name." at the record type) — the analyzer wrapper does surface this sharedName despite no CFE `analyzerCode` entry |
| external_factory_redirection | 1 | authored; official messages.yaml#externalFactoryRedirection example verbatim | live (CFE crosswalk): target code fires, sole row |
| external_getter_with_body | 1 | authored; `external int get x { ... }` | registry-only: oracle emits `external_method_with_body` |
| external_operator_with_body | 1 | authored; `external bool operator ==(...) { ... }` | registry-only: oracle emits `external_method_with_body` |
| external_setter_with_body | 1 | authored; `external set x(int v) { }` | registry-only: oracle emits `external_method_with_body` |
| factory_with_initializers | 1 | authored; factory constructor with `: x = 1` initializer list | registry-only: 9-row recovery cascade (expected_class_member/expected_token/missing_function_body/...) |
| final_constructor | 1 | authored; `final C()` | registry-only: oracle emits `extraneous_modifier` |
| final_method | 1 | authored; `final void m()` | registry-only: oracle emits `extraneous_modifier` |
| getter_in_function | 1 | lifted pin `getter_in_function_body_recovers_with_the_oracle_pair` | registry-only: oracle pair `expected_token` + `missing_function_parameters` (+ unused-element warnings), matching the frozen pin row set |
| invalid_constant_const_prefix | 1 | lifted dark pin (`case const 1:`) | live (CFE crosswalk): target code fires, sole row |
| invalid_constant_pattern_binary | 1 | lifted dark pin (`case 1 + 2:`) | live (CFE crosswalk): target code fires, sole row |
| invalid_constant_pattern_unary | 1 | lifted dark pin (`case !false:`) | live (CFE crosswalk): target code fires, sole row |
| invalid_generic_function_type | 1 | authored; `typedef F = void Function<T>;` (no parameter list) | registry-only: oracle emits `expected_token` + `missing_function_parameters` + `missing_function_body` |
| invalid_language_version_override | 2 | authored; lowercase-`dart` override comment per language_version_override_verifier.dart; /2 valid control | live (analyzer-side verifier, WARNING): /1 fires, /2 silent |
| invalid_star_after_async | 1 | lifted group-A dead-code pin (`f() async* => 1;`) | registry-only: oracle emits `return_in_generator` (not the generic family — guess corrected) |
| invalid_sync | 1 | lifted group-A dead-code pin (`f() sync => 1;`) | registry-only: oracle emits `missing_star_after_sync` |
| invalid_unicode_escape_started | 2 | lifted dark pin (lone backslash before newline); /2 escaped-backslash control | live (scanner face): /1 fires (+ `unterminated_string_literal`), /2 silent |
| invalid_use_of_identifier_augmented | 1 | authored; `print(augmented)` in a plain library | registry-only: oracle emits `undefined_identifier` |
| literal_with_class_and_new | 1 | lifted dark pin (`new List<int>[1]`) | live (CFE crosswalk): target code fires (+ `unused_local_variable`) |
| local_function_declaration_modifier | 1 | lifted group-A dead-code pin (`static f() {}` local) | registry-only: oracle emits `extraneous_modifier`, matching the frozen pin row set |
| missing_closing_parenthesis | 1 | authored; unclosed formal-parameter group | registry-only: oracle emits `expected_token` |
| missing_expression_in_initializer | 1 | authored; `int x = ;` | registry-only: oracle emits `missing_identifier` |
| missing_function_keyword | 1 | authored; `typedef F = int (int x);` | registry-only: oracle emits `expected_token` |
| missing_get | 1 | lifted pin fixture blindspot_accessors (`int x => 1;` member) | registry-only: oracle emits `missing_method_parameters`, matching the frozen pin row set |
| missing_hex_digit | 2 | authored; `0x` with no digits; /2 `0xFF` control | live (scanner face): /1 fires, /2 silent except `unused_local_variable` |
| missing_name_for_named_parameter | 1 | authored; `void f({int})` | SILENT: a type-only named parameter is legal in the pinned oracle — case pins the acceptance boundary |
| missing_name_in_library_directive | 1 | authored; bare `library;` (legal via unnamed libraries in current Dart) | SILENT as predicted: documents the boundary against the registry-only code |
| missing_name_in_part_of_directive | 1 | authored; `part of;` | registry-only: oracle emits `expected_string_literal` |
| missing_quote | 1 | lifted pin fixture blindspot_library_directives (`import 'a.dart`), companion keeps the recovered URI resolvable | never-fires by construction (fixture-documented gap): oracle emits `unterminated_string_literal` + `expected_token` |
| missing_variable_in_for_each | 1 | lifted group-A dead-code pin (`for (in xs)`) | registry-only: oracle emits `expected_identifier_but_got_keyword` + `expected_token` + `undefined_identifier` |
| mixed_parameter_groups | 1 | authored; `[int b], {int c}` in one list | registry-only: the parameter-group codes have no CFE emission; oracle emits `expected_token` + `missing_default_value_for_parameter` |
| multiple_clauses | 1 | lifted dark pin (`enum E implements Foo implements Bar`) | live (CFE crosswalk): target code fires, sole row |
| multiple_named_parameter_groups | 1 | authored; `{int a}, {int b}` | registry-only: oracle emits `expected_token` + `missing_default_value_for_parameter` |
| multiple_positional_parameter_groups | 1 | authored; `[int a], [int b]` | registry-only: oracle emits `expected_token` + `missing_default_value_for_parameter` |
| native_clause_should_be_annotation | 1 | authored; official messages.yaml#nativeClauseShouldBeAnnotation shape (`class A native "x"`) | probed retired, run confirms: target silent, oracle emits `native_clause_in_non_sdk_code` |
| non_constructor_factory | 1 | authored; `factory m()` inside `class C` | registry-only: oracle emits `new_with_undefined_constructor_default` |
| non_identifier_library_name | 1 | lifted pin fixture blindspot_library_directives (`library 1;`) | registry-only: oracle emits `missing_identifier`, matching the frozen pin row set |
| non_string_literal_as_uri | 1 | lifted pin fixture blindspot_library_directives (`import dart:core;`) | registry-only: 5-row recovery cascade led by `expected_string_literal`, matching the frozen pin row set |
| normal_before_optional_parameters | 1 | authored; `[int a], int b` | registry-only: oracle emits `expected_token` + `missing_default_value_for_parameter` |
| out_of_order_clauses | 2 | lifted dark pin (`implements Foo with Bar` on an enum); /2 canonical-order control | live (CFE crosswalk): /1 fires, sole row; /2 silent of the target (`class_used_as_mixin` warning only) |
| positional_parameter_outside_group | 1 | authored; `{int a}, int b` | registry-only: oracle emits `expected_token` + `missing_default_value_for_parameter` |
| setter_in_function | 1 | lifted pin fixture blindspot_accessors (`set s(int v) {}` local) | registry-only: oracle emits `unexpected_token`, matching the frozen pin row set |
| stack_overflow | 1 | lifted pin `deep_record_type_matches_oracle_per_level_rows` (600 nested record-type parens; the official parser has no cap on the type-descent path) | FINDING: the oracle does not latch stackOverflow here — it iterates and emits 600 `record_type_one_positional_no_trailing_comma` rows (one per paren), clean and bounded; dartq matches with no observable cap (stack-safety crash guard far above oracle-reachable depths); see risks |
| static_getter_without_body | 1 | lifted pin fixture blindspot_accessors (`static int get x;`) | registry-only: oracle emits `missing_function_body`, matching the frozen pin row set |
| static_setter_without_body | 1 | lifted pin fixture blindspot_accessors (`static set x(int v);`) | registry-only: oracle emits `missing_function_body`, matching the frozen pin row set |
| type_arguments_on_type_variable | 1 | authored; official messages.yaml#typeArgumentsOnTypeVariable example verbatim | CLI surface differs: oracle emits the resolution face `wrong_number_of_type_arguments`, not the CFE parser code |
| type_parameter_on_constructor | 1 | authored; official messages.yaml#constructorWithTypeParameters example verbatim | live (CFE crosswalk): target code fires, sole row |
| type_parameter_on_operator | 1 | authored; official messages.yaml#operatorWithTypeParameters example verbatim | live (CFE crosswalk): target code fires, sole row |
| unterminated_multi_line_comment | 2 | authored scanner shapes (unclosed `/*` at EOF; /2 closed control) | live (scanner face): /1 fires, /2 silent |
| var_as_type_name | 1 | authored; official messages.yaml#varAsTypeName example verbatim (`Map<String, var>`) | live (CFE crosswalk): target code fires; the official `exampleAllowOtherCodes` extra rows materialized (`non_type_as_type_argument`, `not_initialized_non_nullable_instance_field`) and stay part of the 1:1 target |
| var_class | 1 | lifted pin `var_class_missing_name_reparse` (`var class VarClass {}`) | registry-only: oracle pair `missing_identifier` + `expected_token` (+ unused-element warning), matching the frozen pin row set |
| var_enum | 1 | authored; sibling of the var_class/var_typedef reparse shapes | registry-only: same reparse pair as var_class |
| var_typedef | 1 | lifted pin `var_typedef_missing_name_reparse` (`var typedef VarTypedef = ...`) | registry-only: same reparse pair, matching the frozen pin row set |

### Syntax-batch risks

1. **stack_overflow/1 divergence surface**: the 3.13.7 oracle handles the
   600-deep record-type nesting iteratively (600
   `record_type_one_positional_no_trailing_comma` rows, 13.7s chunk total, no
   crash/hang), while the dartq parser pin asserts a parser-level
   `stack_overflow` bailout on the same input. The case freezes the oracle row
   set; reconciling dartq's surfaced rows with it is engine-side parity work,
   exactly what this case exists to catch.
2. **annotation_with_type_arguments**: the official example's `// @dart=2.12`
   pin works — the gate face (`experiment_not_enabled`) fires and the target
   code stays unexercised in the CLI. Covering the target code itself would
   need a context where the analyzer wrapper runs with generic-metadata
   enabled but rejects the annotation; none found in this SDK generation.
3. **Registry-only codes**: the ~29 codes marked registry-only above cannot
   appear in oracle output by construction; the parity value is the frozen
   resolution/recovery row set (several already frozen in the pin fixtures).
   This is a documented upstream gap, not a corpus defect.
4. **Silence boundaries**: `library;` (unnamed libraries) and `void f({int})`
   (type-only named parameter) are legal in the pinned oracle — both silence
   cases pin those acceptance boundaries against false positives.

## Case risks

1. `plugins_in_inner_options/1`: only fires if the oracle processes the nested
   `lib/analysis_options.yaml` as its own analysis context. If the harness only
   reports the root context, the case yields zero diagnostics — still
   parity-safe (dartq must emit nothing too), but low value.
2. `included_file_warning/1`: the included file's undefined-lint may surface
   both natively and wrapped at the include site; the walker implementation
   supports either observable shape, oracle output decides.
3. `subtype_of_struct_class/3`: emits the shared code twice (the `with` clause
   on C and the `implements` clause on the mixin class S2 — any type usable in
   a with clause is itself a compound subtype and fires on its own
   declaration). Same registry code for both, so coverage is unaffected.
4. `ffi_native_invalid_duplicate_default_asset/1`: annotations on the library
   directive reference `DefaultAsset` imported below the directive — this is
   the official documented ordering, but it leans on whole-unit import scoping.
5. `text_direction_code_point_in_comment` / `..._in_literal`: fixtures embed a
   literal U+202E byte; any re-encoding of the corpus directory must stay UTF-8.
6. Pubspec/manifest family (#2721): every manifest case enables the gate via
   `analyzer: optional-checks: chrome-os-manifest-checks: true`; under the bare
   `analyzer:` key the option itself reports `unsupported_option` instead.
   Manifest `/1` shapes keep `touchscreen` at `required="false"` so only the
   target code fires (a manifest without it always also reports
   `no_touchscreen_feature`).
7. `multiple_plugins/1` and `/2` use the legacy `analyzer: plugins:` key, so
   both also fire `analysis_options_deprecated_plugins`; `/2` pins the
   boundary that same-name duplicates do not conflict. The target code itself
   did **not** surface in the oracle run: its report sites sit behind the
   include walker's legacy-plugin path
   (`_ParsedLegacyPluginsData.reportLocalMultiplePlugins`), which produced no
   `multiple_plugins` diagnostic for the plain-list shape in this pin's CLI
   output. The two cases stay as parity cases (both sides must emit only the
   deprecated-plugins warning); covering the code itself needs the
   include-chain shape (`reportMultiplePluginsWithIncluded`), left to a
   follow-up lane.
8. `path_does_not_exist` is shared by the dependency, screenshot and
   workspace validators; the two cases cover the dependency face, with the
   existing-directory negative also passing the pubspec-presence check.
9. Messages that embed a case's absolute package root (the workspace
   validator's `workspace_value_not_subdirectory`, the options walker's
   include diagnostics surfaced by include_file_not_found /
   included_file_warning / recursive_include_file) are normalized by the
   runner: attributed rows replace the case root with `<chunk>` on both the
   oracle and the dartq side, keeping the rows deterministic across chunk
   dirs and comparable between the two.
