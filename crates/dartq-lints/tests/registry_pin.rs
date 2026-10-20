//! Registry completeness pin: every wave-1 rule must exist in the official
//! `package:lints` sets, with its [`Group`] matching the set it came from.
//!
//! The lists below are pinned verbatim from the pinned `package:lints`
//! snapshot (`core.yaml` / `recommended.yaml`, upstream dart-lang/lints);
//! re-pin them when the snapshot updates. When that happens, update the
//! lists here in the same change as the registry.
//!
//! Also the governance pin for the frozen lint-gate universe
//! (`tests/harness/lint_codes.txt`, the `--codes-file` of
//! `scripts/lint-gate.sh`): the freeze must stay exactly the set of
//! `package:lints` rules this registry can emit minus the divergence
//! ledger's `gate_status=excluded` rows (`tests/harness/lint_divergences.tsv`,
//! the source of truth for gate exclusions) — no orphans (rows the
//! implementation cannot emit), no gaps (mounted rules the gate would never
//! judge and no ledger row rules out), no rot (excluded rules back in the
//! freeze). A stale freeze fails here with the offending rows named.

use std::collections::BTreeSet;
use std::path::Path;

use dartq_lints::{
    EnableSet, Group, SEMANTIC_RULES, emittable_rule_names, mounts_semantics, rule_by_name, rules,
};

/// The pinned `package:lints` snapshot's `core.yaml` — the critical set,
/// pinned verbatim.
const OFFICIAL_CORE: &[&str] = &[
    "avoid_empty_else",
    "avoid_relative_lib_imports",
    "avoid_shadowing_type_parameters",
    "avoid_types_as_parameter_names",
    "await_only_futures",
    "camel_case_extensions",
    "camel_case_types",
    "collection_methods_unrelated_type",
    "curly_braces_in_flow_control_structures",
    "dangling_library_doc_comments",
    "depend_on_referenced_packages",
    "empty_catches",
    "file_names",
    "hash_and_equals",
    "implicit_call_tearoffs",
    "library_annotations",
    "no_duplicate_case_values",
    "no_wildcard_variable_uses",
    "non_constant_identifier_names",
    "null_check_on_nullable_type_parameter",
    "prefer_generic_function_type_aliases",
    "prefer_is_empty",
    "prefer_is_not_empty",
    "prefer_iterable_whereType",
    "prefer_typing_uninitialized_variables",
    "provide_deprecation_message",
    "secure_pubspec_urls",
    "type_literal_in_constant_pattern",
    "unintended_html_in_doc_comment",
    "unnecessary_overrides",
    "unrelated_type_equality_checks",
    "use_string_in_part_of_directives",
    "valid_regexps",
    "void_checks",
];

/// The pinned `package:lints` snapshot's `recommended.yaml` — the idiom set
/// (includes core via `include:`), pinned verbatim.
const OFFICIAL_RECOMMENDED: &[&str] = &[
    "annotate_overrides",
    "avoid_function_literals_in_foreach_calls",
    "avoid_init_to_null",
    "avoid_renaming_method_parameters",
    "avoid_return_types_on_setters",
    "avoid_returning_null_for_void",
    "avoid_single_cascade_in_expression_statements",
    "constant_identifier_names",
    "control_flow_in_finally",
    "empty_constructor_bodies",
    "empty_statements",
    "exhaustive_cases",
    "implementation_imports",
    "invalid_runtime_check_with_js_interop_types",
    "library_prefixes",
    "library_private_types_in_public_api",
    "no_leading_underscores_for_library_prefixes",
    "no_leading_underscores_for_local_identifiers",
    "null_closures",
    "overridden_fields",
    "package_names",
    "prefer_adjacent_string_concatenation",
    "prefer_collection_literals",
    "prefer_conditional_assignment",
    "prefer_contains",
    "prefer_final_fields",
    "prefer_for_elements_to_map_fromIterable",
    "prefer_function_declarations_over_variables",
    "prefer_if_null_operators",
    "prefer_initializing_formals",
    "prefer_inlined_adds",
    "prefer_interpolation_to_compose_strings",
    "prefer_is_not_operator",
    "prefer_null_aware_operators",
    "prefer_spread_collections",
    "recursive_getters",
    "slash_for_doc_comments",
    "type_init_formals",
    "unnecessary_brace_in_string_interps",
    "unnecessary_const",
    "unnecessary_constructor_name",
    "unnecessary_getters_setters",
    "unnecessary_late",
    "unnecessary_library_name",
    "unnecessary_new",
    "unnecessary_null_aware_assignments",
    "unnecessary_null_in_if_null_operators",
    "unnecessary_nullable_for_final_variable_declarations",
    "unnecessary_string_escapes",
    "unnecessary_string_interpolations",
    "unnecessary_this",
    "unnecessary_to_list_in_spreads",
    "use_function_type_syntax_for_parameters",
    "use_rethrow_when_possible",
    "use_super_parameters",
];

/// The analyzer-diagnostic mirrors — not `package:lints` members. The
/// diagnostic registry carries each machine name
/// (`pkg/analyzer/lib/src/diagnostic/diagnostic.g.dart`):
/// `unnecessary_type_check_true` / `unnecessary_type_check_false` (the
/// linter-side rule file left the tree in the Dart 3.15 main re-pin) and
/// the pubspec dependency validator's `path:` family
/// (`path_not_posix` :14004, `path_does_not_exist` :13987,
/// `path_pubspec_does_not_exist` :14020 — one validator, upstream Dart
/// SDK source,
/// `pkg/analyzer/lib/src/pubspec/validators/dependency_validator.dart`).
const OFFICIAL_DIAGNOSTIC_MIRRORS: &[&str] = &[
    "path_does_not_exist",
    "path_not_posix",
    "path_pubspec_does_not_exist",
    "unnecessary_type_check",
];

/// The `removed` state of the published universe (`rules.json`, 14
/// entries): the SDK still registers each as `RemovedAnalysisRule`
/// (`pkg/analyzer/lib/src/analysis_rule/analysis_rule.dart:385`,
/// `diagnosticCodes => const []`), pinned verbatim in the alphabetical
/// wave-1 registration order.
const OFFICIAL_REMOVED: &[&str] = &[
    "always_require_non_null_named_parameters",
    "avoid_as",
    "avoid_returning_null",
    "avoid_returning_null_for_future",
    "avoid_unstable_final_fields",
    "enable_null_safety",
    "invariant_booleans",
    "iterable_contains_unrelated_type",
    "list_remove_unrelated_type",
    "package_api_docs",
    "prefer_bool_in_asserts",
    "prefer_equal_for_default_values",
    "super_goes_last",
    "unsafe_html",
];

/// The `Standalone` rules — registered SDK rules outside the
/// `package:lints` sets, enabled only through the options face. One pinned
/// list per wave in wave-registration order
/// (alphabetical within a wave); each wave appends its block here, and the
/// registry section must match the concatenation byte-for-byte.
const OFFICIAL_STANDALONE_WAVE2: &[&str] = &[
    "always_declare_return_types",
    "always_put_control_body_on_new_line",
    "always_put_required_named_parameters_first",
    "avoid_annotating_with_dynamic",
    "avoid_catches_without_on_clauses",
    "avoid_classes_with_only_static_members",
    "avoid_double_and_int_checks",
    "avoid_escaping_inner_quotes",
    "avoid_final_parameters",
    "avoid_js_rounded_ints",
    "avoid_multiple_declarations_per_line",
];

/// The W3 declaration-shape wave (#992, parity-plan wave 3): eleven more
/// registered SDK rules outside the `package:lints` sets, alphabetical.
const OFFICIAL_STANDALONE_WAVE3: &[&str] = &[
    "avoid_private_typedef_functions",
    "avoid_returning_this",
    "avoid_types_on_closure_parameters",
    "avoid_unused_constructor_parameters",
    "combinators_ordering",
    "initialize_in_field_declaration",
    "leading_newlines_in_multiline_strings",
    "migrate_design_widgets",
    "missing_whitespace_between_adjacent_strings",
    "no_adjacent_strings_in_list",
    "no_literal_bool_comparisons",
];

/// The W4 expression-shape wave (parity-plan wave 4): nine registered SDK
/// rules outside the `package:lints` sets, alphabetical.
const OFFICIAL_STANDALONE_WAVE4: &[&str] = &[
    "avoid_bool_literals_in_conditional_expressions",
    "avoid_null_checks_in_equality_operators",
    "literal_only_boolean_expressions",
    "noop_primitive_operations",
    "prefer_expression_function_bodies",
    "prefer_int_literals",
    "unnecessary_lambdas",
    "unnecessary_parenthesis",
    "unnecessary_statements",
];

/// The W7 name/empty/slice wave (parity-plan wave 7): eight registered SDK
/// rules across the naming, emptiness, and assignment families,
/// alphabetical.
const OFFICIAL_STANDALONE_WAVE7: &[&str] = &[
    "no_self_assignments",
    "one_member_abstracts",
    "prefer_asserts_with_message",
    "sort_constructors_first",
    "sort_unnamed_constructors_first",
    "throw_in_finally",
    "unnecessary_breaks",
    "unnecessary_final",
];

/// The W8 collection wave (parity-plan wave 8): two registered SDK rules,
/// alphabetical.
const OFFICIAL_STANDALONE_WAVE8: &[&str] = &[
    "prefer_foreach",
    "prefer_if_elements_to_conditional_expressions",
];

/// The `require_trailing_commas` face (#1053): the version-gated
/// tall-style rule — registers below 3.7, silent from 3.7 where
/// `dart format` enforces the commas.
const OFFICIAL_STANDALONE_REQUIRE_TRAILING_COMMAS: &[&str] = &["require_trailing_commas"];

/// The strings/directives mini wave: ten registered SDK rules across the
/// string-literal and directive families, alphabetical.
const OFFICIAL_STANDALONE_MINI_WAVE: &[&str] = &[
    "directives_ordering",
    "eol_at_end_of_file",
    "library_names",
    "lines_longer_than_80_chars",
    "package_prefixed_library_names",
    "prefer_double_quotes",
    "prefer_single_quotes",
    "unnecessary_library_directive",
    "unnecessary_raw_strings",
    "use_raw_strings",
];

/// The #1094 batch close (waves A/B/C): seventeen registered SDK rules
/// across the declaration-shape, doc-comment, expression, flow-pattern,
/// type-reference, and collection domains, alphabetical.
const OFFICIAL_STANDALONE_WAVES_ABC: &[&str] = &[
    "document_ignores",
    "empty_container_bodies",
    "flutter_style_todos",
    "invalid_case_patterns",
    "missing_code_block_language_in_doc_comment",
    "prefer_constructors_over_static_methods",
    "prefer_final_parameters",
    "simplify_variable_pattern",
    "strict_top_level_inference",
    "switch_on_type",
    "type_annotate_public_apis",
    "unnecessary_const_in_enum_constructor",
    "unnecessary_type_name_in_constructor",
    "unnecessary_underscores",
    "use_null_aware_elements",
    "use_primary_constructors",
    "var_with_no_type_annotation",
];

/// The W1/W2 closeout promotion (#1108): fifty registered SDK rules across
/// the directives, flow, pubspec, type-reference, flutter, doc-comment, and
/// declaration domains, alphabetical.
const OFFICIAL_STANDALONE_W1_CLOSEOUT: &[&str] = &[
    "always_specify_types",
    "always_use_package_imports",
    "annotate_redeclares",
    "async_return_with_no_await",
    "avoid_catching_errors",
    "avoid_equals_and_hash_code_on_mutable_classes",
    "avoid_field_initializers_in_const_classes",
    "avoid_futureor_void",
    "avoid_implementing_value_types",
    "avoid_positional_boolean_parameters",
    "avoid_setters_without_getters",
    "avoid_unnecessary_containers",
    "avoid_void_async",
    "avoid_web_libraries_in_flutter",
    "conditional_uri_does_not_exist",
    "deprecated_consistency",
    "diagnostic_describe_all_properties",
    "erase_dart_type_extension_types",
    "join_return_with_assignment",
    "no_default_cases",
    "no_logic_in_create_state",
    "no_raw_types",
    "omit_local_variable_types",
    "omit_obvious_local_variable_types",
    "omit_obvious_property_types",
    "prefer_asserts_in_initializer_lists",
    "prefer_const_constructors_in_immutables",
    "prefer_final_in_for_each",
    "prefer_final_locals",
    "prefer_relative_imports",
    "prefer_void_to_null",
    "public_member_api_docs",
    "simple_directive_paths",
    "sized_box_for_whitespace",
    "sized_box_shrink_expand",
    "sort_child_properties_last",
    "sort_pub_dependencies",
    "specify_nonobvious_local_variable_types",
    "specify_nonobvious_property_types",
    "test_types_in_equals",
    "unnecessary_async",
    "unnecessary_primary_constructor_body",
    "use_colored_box",
    "use_declaring_parameters",
    "use_decorated_box",
    "use_full_hex_values_for_flutter_colors",
    "use_key_in_widget_constructors",
    "use_late_for_private_fields_and_variables",
    "use_setters_to_change_properties",
    "use_to_and_as_if_applicable",
];

/// The W1/W2 final tail promotion (#1141): twenty-one registered SDK rules
/// across the expression, slice, and string domains, alphabetical — the
/// last block to ride the aggregated incoming tail before this close
/// promoted it. The open-tail doc's forward note is closed with it:
/// `prefer_null_aware_method_calls` re-homed here at #1141's close, as that
/// note anticipated.
const OFFICIAL_STANDALONE_W2_CLOSEOUT: &[&str] = &[
    "avoid_dynamic_calls",
    "avoid_print",
    "avoid_type_to_string",
    "cascade_invocations",
    "cast_nullable_to_non_nullable",
    "discarded_futures",
    "do_not_use_environment",
    "no_dynamic_casts",
    "no_runtimetype_tostring",
    "only_throw_errors",
    "parameter_assignments",
    "prefer_null_aware_method_calls",
    "unawaited_futures",
    "unnecessary_await_in_return",
    "unnecessary_null_aware_operator_on_extension_on_nullable",
    "unnecessary_null_checks",
    "unnecessary_unawaited",
    "use_if_null_to_convert_nulls_to_bools",
    "use_is_even_rather_than_modulo",
    "use_string_buffers",
    "use_truncating_division",
];

/// The W5 closeout promotion (#1153): nineteen registered SDK rules across
/// the collections, declarations, docs, expressions, flow, names, and type
/// domains, alphabetical — the last block to ride the aggregated incoming
/// tail before this close promoted it.
const OFFICIAL_STANDALONE_W5_CLOSEOUT: &[&str] = &[
    "avoid_redundant_argument_values",
    "avoid_slow_async_io",
    "cancel_subscriptions",
    "close_sinks",
    "comment_references",
    "deprecated_member_use_from_same_package",
    "future_sync_value",
    "matching_super_parameters",
    "prefer_const_constructors",
    "prefer_const_declarations",
    "prefer_const_literals_to_create_immutables",
    "prefer_mixin",
    "remove_deprecations_in_breaking_versions",
    "tighten_type_of_initializing_formals",
    "unnecessary_ignore",
    "unnecessary_this_alias",
    "use_enums",
    "use_named_constants",
    "use_test_throws_matchers",
];

/// The W6 internal wave (#1157 L1): the nine `RuleState.internal()`
/// SDK-dogfooding rules, alphabetical. Upstream registers them
/// unconditionally and enablement is name-only (`registry.dart:117-137`),
/// so naming one in `analysis_options` silently enables it — registering
/// the names honors that face. They never fire outside SDK sources
/// (every firing path's antecedent is an SDK-internal artifact),
/// so each also sits in `STRUCTURALLY_SILENT` with a
/// divergence-ledger `excluded` row.
const OFFICIAL_STANDALONE_W6_INTERNAL: &[&str] = &[
    "analyzer_element_model_tracking_bad",
    "analyzer_element_model_tracking_more_than_one",
    "analyzer_element_model_tracking_zero",
    "analyzer_public_api_bad_part_directive",
    "analyzer_public_api_bad_type",
    "analyzer_public_api_experimental_inconsistency",
    "analyzer_public_api_exports_non_public_name",
    "analyzer_public_api_impl_in_public_api",
    "analyzer_to_be_deprecated_use",
];

/// The W6 triggerable wave (#1157, closed at #1213): the four rules that
/// fire on user code — the two experimental variance-domain rules and the
/// two process-adjacent stable rules — in one byte-sorted const. The sort
/// is the pin's byte-order trap: `unreachable_from_main` sorts BEFORE
/// `unsafe_variance` (`unr` < `uns`), so the wave list is one merged
/// table, never a per-lane append. Both experimental rules register
/// unconditionally upstream (`RuleState.experimental()` is registry
/// metadata, not an enable gate) and all four enable by name only; none
/// joins `STRUCTURALLY_SILENT`.
const OFFICIAL_STANDALONE_W6_TRIGGERABLE: &[&str] = &[
    "implicit_reopen",
    "unreachable_from_main",
    "unsafe_variance",
    "use_build_context_synchronously",
];

/// The standalone universe in wave order — the registry-section contract.
fn official_standalone() -> Vec<&'static str> {
    let mut all = OFFICIAL_STANDALONE_WAVE2.to_vec();
    all.extend(OFFICIAL_STANDALONE_WAVE3);
    all.extend(OFFICIAL_STANDALONE_WAVE4);
    all.extend(OFFICIAL_STANDALONE_WAVE7);
    all.extend(OFFICIAL_STANDALONE_WAVE8);
    all.extend(OFFICIAL_STANDALONE_REQUIRE_TRAILING_COMMAS);
    all.extend(OFFICIAL_STANDALONE_MINI_WAVE);
    all.extend(OFFICIAL_STANDALONE_WAVES_ABC);
    all.extend(OFFICIAL_STANDALONE_W1_CLOSEOUT);
    all.extend(OFFICIAL_STANDALONE_W2_CLOSEOUT);
    all.extend(OFFICIAL_STANDALONE_W5_CLOSEOUT);
    all.extend(OFFICIAL_STANDALONE_W6_INTERNAL);
    all.extend(OFFICIAL_STANDALONE_W6_TRIGGERABLE);
    all
}

#[test]
fn rule_names_are_unique() {
    let mut seen: Vec<&str> = Vec::with_capacity(rules().len());
    for spec in rules() {
        assert!(
            seen.iter().all(|s| *s != spec.name),
            "duplicate rule name {}",
            spec.name
        );
        seen.push(spec.name);
    }
}

/// The completeness pin: a registry entry that leaves the official sets (or
/// carries the wrong `Group`) breaks the crate's contract. Membership is
/// case-insensitive like [`rule_by_name`]: the pinned `package:lints` yaml
/// keeps two mixed-case spellings (`prefer_iterable_whereType`,
/// `prefer_for_elements_to_map_fromIterable`) while the registered names —
/// the emitted code strings — are their all-lowercase machine forms.
#[test]
fn every_rule_is_in_its_official_set() {
    fn contains(official: &[&str], name: &str) -> bool {
        official
            .iter()
            .any(|entry| entry.eq_ignore_ascii_case(name))
    }
    for spec in rules() {
        let official = match spec.group {
            Group::Core => OFFICIAL_CORE,
            Group::Recommended => OFFICIAL_RECOMMENDED,
            Group::Diagnostic => OFFICIAL_DIAGNOSTIC_MIRRORS,
            Group::Removed => OFFICIAL_REMOVED,
            Group::Standalone => {
                assert!(
                    contains(&official_standalone(), spec.name),
                    "{} claims the standalone group but is absent from the wave lists",
                    spec.name
                );
                continue;
            }
        };
        assert!(
            contains(official, spec.name),
            "{} claims group {:?} but is absent from the official list",
            spec.name,
            spec.group
        );
    }
}

/// The recommended set includes core (`include: package:lints/core.yaml`),
/// so a Recommended entry must not also live in core — that would mean it
/// was registered with the wrong group.
#[test]
fn recommended_rules_are_not_core_rules() {
    for spec in rules() {
        if spec.group == Group::Recommended {
            assert!(
                !OFFICIAL_CORE
                    .iter()
                    .any(|entry| entry.eq_ignore_ascii_case(spec.name)),
                "{} is a core-set rule but registered as Recommended",
                spec.name
            );
        }
    }
}

/// [`EnableSet`] stores five 64-bit words; the registry must stay within
/// them (five words carry the full 269-rule SDK registry plus the dartq
/// mirror). Grow `WORDS` together with this bound when the set outgrows a
/// word.
#[test]
fn registry_fits_the_enable_set_word_count() {
    assert!(
        rules().len() <= 320,
        "registry holds {} rules; EnableSet has five 64-bit words",
        rules().len()
    );
}

/// The slice pin: 89 `package:lints` members through #586 (34 core + 55
/// recommended-only, no member left blocked or documented-out), the #570
/// diagnostic mirror, the #986 removed-registry no-ops (re-enumerated at
/// the #1025 reconciliation: rules.json `removed` holds exactly 14), the
/// #987 W2 standalone wave, the #992 W3 wave, the batch close of the
/// W4/W7/W8 waves, the `require_trailing_commas` face, and the
/// strings/directives mini wave (156), and the #1094 batch close of the
/// waves A/B/C declaration-shape, doc-comment, expression, flow-pattern,
/// type-reference, and collection leftovers (17 more) make 173, and the
/// W1/W2 closeout promotion (#1108) of the directives, flow, pubspec,
/// type-reference, flutter, doc-comment, and declaration incoming slices
/// (50 more) makes the 223-rule skeleton, and the final tail promotion
/// (#1141) of the expressions, slice, and strings incoming slices (21 more)
/// makes 244, and the final W5 closeout promotion (#1153) of the W5
/// (#1124) plus #1150/#1151/#1152 incoming slices (19 more, two of them
/// structurally silent) makes 263 with the aggregated tail empty, and the
/// #1157 wave L1 face (#1194) lands the nine `RuleState.internal`
/// SDK-dogfooding rules (all structurally silent) onto the declarations
/// incoming slice, and the #1157 wave L2+L3 face (#1197) lands the two
/// variance-domain rules — `implicit_reopen` onto the declarations slice,
/// `unsafe_variance` onto the type-reference slice — 274, and the W6
/// closeout (#1213) lands the final two triggerable rules —
/// `unreachable_from_main` onto the declarations domain,
/// `use_build_context_synchronously` onto the flow domain — and promotes
/// the whole thirteen-name W6 block into the frozen skeleton with the
/// aggregated tail empty again: 276, the wave's final count, and the
/// pubspec path-validator tranche (R780 #1521) lands the analyzer
/// dependency validator's three `path:` mirrors onto the frozen
/// diagnostic-mirror section: 279.
#[test]
fn registry_holds_the_pinned_count() {
    assert_eq!(rules().len(), 279);
}

/// The registry-order contract (parity-plan R8): one contiguous block per
/// section, sections in wave order — core, recommended-only, diagnostic
/// mirrors, then the wave sections (the removed block first) — and each
/// section exactly its pinned official set. The wave sections pin exact
/// order (each new wave appends its section here); the legacy
/// core/recommended blocks keep their historical registration order, each
/// entry's identity pinned by `rule_by_name_round_trips`. Comparisons run
/// on lowercased forms: the pinned `package:lints` yaml keeps two
/// mixed-case spellings while the registered names are the all-lowercase
/// machine code strings.
#[test]
fn registry_sections_hold_the_official_order() {
    fn official(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|name| name.to_ascii_lowercase()).collect()
    }
    let ranks: Vec<u8> = rules()
        .iter()
        .map(|spec| match spec.group {
            Group::Core => 0u8,
            Group::Recommended => 1,
            Group::Diagnostic => 2,
            Group::Removed => 3,
            Group::Standalone => 4,
        })
        .collect();
    assert!(
        ranks.windows(2).all(|pair| pair[0] <= pair[1]),
        "registry sections must be contiguous, in wave order"
    );

    let section = |group: Group| -> BTreeSet<String> {
        rules()
            .iter()
            .filter(|spec| spec.group == group)
            .map(|spec| spec.name.to_ascii_lowercase())
            .collect()
    };
    assert_eq!(section(Group::Core), official(OFFICIAL_CORE));
    assert_eq!(section(Group::Recommended), official(OFFICIAL_RECOMMENDED));
    assert_eq!(
        section(Group::Diagnostic),
        official(OFFICIAL_DIAGNOSTIC_MIRRORS)
    );

    let ordered_section = |group: Group| -> Vec<String> {
        rules()
            .iter()
            .filter(|spec| spec.group == group)
            .map(|spec| spec.name.to_ascii_lowercase())
            .collect()
    };
    let ordered_official = |names: &[&str]| -> Vec<String> {
        names.iter().map(|name| name.to_ascii_lowercase()).collect()
    };
    assert_eq!(
        ordered_section(Group::Removed),
        ordered_official(OFFICIAL_REMOVED)
    );
    assert_eq!(
        ordered_section(Group::Standalone),
        ordered_official(&official_standalone())
    );
}

/// The diagnostic-code-string invariant: every registered name IS the
/// official machine code string — all-lowercase, matching the upstream
/// `LintNames` constants (`pkg/linter/lib/src/lint_names.g.dart`, every
/// constant lowercase) and `tool/machine/rules.json` (all 264 entries
/// lowercase) byte-for-byte. The JSON writer emits `code.name` verbatim and
/// the machine writer its uppercase, so `spec.name` is the single source of
/// the emitted code on both faces. The two `package:lints` yaml spellings
/// (`prefer_iterable_whereType`,
/// `prefer_for_elements_to_map_fromIterable` — yaml conventions, not
/// machine names) resolve through the case-insensitive [`rule_by_name`]
/// like every other casing; they are not the registered form.
#[test]
fn registered_names_are_official_code_strings() {
    for spec in rules() {
        assert_eq!(
            spec.name,
            spec.name.to_ascii_lowercase(),
            "{} must be the all-lowercase official diagnostic code string",
            spec.name
        );
    }
    // The two former yaml spellings stay resolvable — analysis_options
    // files authored against `package:lints` naming keep working — but the
    // registered (emitted) names are the machine forms.
    for (yaml, code) in [
        ("prefer_iterable_whereType", "prefer_iterable_wheretype"),
        (
            "prefer_for_elements_to_map_fromIterable",
            "prefer_for_elements_to_map_fromiterable",
        ),
    ] {
        let id = rule_by_name(code).unwrap_or_else(|| panic!("{code} must stay registered"));
        assert_eq!(rules()[id.0 as usize].name, code);
        assert_eq!(rule_by_name(yaml), Some(id), "{yaml} must still resolve");
    }
}

#[test]
fn rule_by_name_round_trips() {
    for (i, spec) in rules().iter().enumerate() {
        let id = rule_by_name(spec.name).expect("registered name");
        assert_eq!(id.0 as usize, i, "{} sits at a stale index", spec.name);
    }
    assert_eq!(rule_by_name("no_such_lint_name"), None);
}

/// The official registry lowercases every name (`registry.dart:42,53,133`),
/// so any casing of a registered name resolves — the SDK machine lowercase
/// form and the `package:lints` mixed-case yaml spelling are the same rule,
/// and only truly unknown names error (the `undefined_lint` face).
#[test]
fn rule_by_name_is_case_insensitive() {
    let canonical = rule_by_name("prefer_iterable_wheretype").expect("registered name");
    for spelling in [
        "prefer_iterable_whereType",
        "PREFER_ITERABLE_WHERETYPE",
        "Prefer_Iterable_WhereType",
    ] {
        assert_eq!(rule_by_name(spelling), Some(canonical), "{spelling}");
    }
    assert_eq!(rule_by_name("prefer_iterable_wheretypeX"), None);
}

#[test]
fn enable_set_all_and_resolve_agree() {
    let all = EnableSet::all();
    for i in 0..u16::try_from(rules().len()).expect("rules fit u16") {
        assert!(all.is_enabled(dartq_lints::RuleId(i)));
    }
    let (resolved, unknown) = EnableSet::resolve(rules().iter().map(|s| s.name));
    assert!(unknown.is_empty());
    assert_eq!(resolved, all);
}

// —— the semantic-fold mount predicate (`SEMANTIC_RULES` / `mounts_semantics`) ——

/// The mount's membership pin: `SEMANTIC_RULES` is exactly the rules whose
/// run consumes the per-file semantic fold — every rule whose check
/// call-graph reads `Check::semantics()` (directly or through a shared
/// semantics-reading helper), plus `use_build_context_synchronously` (the
/// fold-fact consumer). Membership drift is a correctness break in both
/// directions: a missing name silently drops that rule's production rows
/// (the fold never mounts), a stale name mounts the fold for nothing.
#[test]
fn semantic_rules_membership() {
    // Every name is a registered rule, once — the derivation is name-based
    // and a duplicate or dangling name is a stale edit.
    let mut seen = BTreeSet::new();
    for name in SEMANTIC_RULES {
        assert!(
            rule_by_name(name).is_some(),
            "SEMANTIC_RULES names unregistered rule {name}"
        );
        assert!(seen.insert(*name), "SEMANTIC_RULES repeats {name}");
    }

    // The named members the mount was built for: the three names the old
    // hand list carried, the hard-gated semantic rules it missed, and the
    // shared-helper consumers (the transitive reads a name list cannot
    // see). A regression here is the mount silently reverting to a partial
    // set.
    for name in [
        "prefer_null_aware_method_calls",
        "unnecessary_lambdas",
        "use_build_context_synchronously",
        "matching_super_parameters",
        "avoid_slow_async_io",
        "comment_references",
        "future_sync_value",
        "unnecessary_this_alias",
        "unnecessary_to_list_in_spreads",
        "unnecessary_parenthesis",
        "avoid_unnecessary_containers",
        "type_init_formals",
    ] {
        assert!(seen.contains(name), "SEMANTIC_RULES lost {name}");
    }

    // The membership contract both ways: a rule in the table mounts the
    // fold when enabled alone; a rule outside it does not. One pass per
    // registered rule — the full enabled-set × property product.
    let semantic: BTreeSet<&str> = SEMANTIC_RULES.iter().copied().collect();
    for spec in rules() {
        let (set, _) = EnableSet::resolve([spec.name]);
        if semantic.contains(spec.name) {
            assert!(
                mounts_semantics(&set),
                "{} consumes the fold but mounts nothing — missing from SEMANTIC_RULES",
                spec.name
            );
        } else {
            assert!(
                !mounts_semantics(&set),
                "{} mounts the fold without consuming it — a stale SEMANTIC_RULES entry \
                 (a new rule that reads `Check::semantics()` must join the table; one that \
                 does not must stay out so corpora pay no fold cost for it)",
                spec.name
            );
        }
    }
}

/// The empty and non-semantic sets mount nothing — the zero-fold cost the
/// gate exists to keep (a file whose enabled set touches no consumer runs
/// the plain run exactly as before).
#[test]
fn mounts_semantics_stays_off_without_consumers() {
    assert!(!mounts_semantics(&EnableSet::none()));
    let (set, _) = EnableSet::resolve(["empty_statements", "file_names"]);
    assert!(!mounts_semantics(&set));
}

/// Every template renders: problem texts are non-empty and `{n}` slots fill
/// (the two-variant rules pass their texts explicitly, so `report_texts`
/// covers them — this pins the registry-level templates only). Literal
/// braces in prose (e.g. `'{}'` in `empty_constructor_bodies`) are fine; only
/// unfilled `{n}` slots fail.
#[test]
fn templates_are_well_formed() {
    for spec in rules() {
        assert!(
            !spec.problem.is_empty(),
            "{} has no problemMessage",
            spec.name
        );
        let rendered = spec.render_problem(&["X"]);
        assert!(!rendered.contains("{0}"), "{} left a raw slot", spec.name);
        if spec.correction.is_empty() {
            assert_eq!(spec.render_correction(&[]), None);
        } else {
            assert!(spec.render_correction(&["X"]).is_some());
        }
    }
}

// —— the frozen lint-gate universe (tests/harness/lint_codes.txt) ———

/// The freeze file `scripts/lint-gate.sh` scopes the INFO x LINT run with
/// (`--codes-file`); relative to this crate's manifest.
const LINT_CODES_FREEZE: &str = "../../tests/harness/lint_codes.txt";

/// The divergence ledger `tests/harness/lint_divergences.tsv` — the source
/// of truth for gate exclusions; relative to this crate's manifest.
const LINT_DIVERGENCES: &str = "../../tests/harness/lint_divergences.tsv";

/// Set rules that cannot emit and are therefore consciously out of the
/// freeze (each also needs a `lint_divergences.tsv` `excluded` row — the
/// gate preflight's A3). Must stay exactly equal to the registered set
/// rules that no `emittable_rule_names` mount face reaches — no AST
/// dispatch (`kinds` empty) and no membership in the pubspec-pass,
/// run-epilogue, or late-join seams. The whole set is the nine
/// `RuleState.internal` SDK-dogfooding rules (#1157 wave L1, #1194) —
/// registered to honor explicit naming, with no firing path outside SDK
/// sources. The two empty-`kinds` names the #1153 closeout once parked
/// here have since mounted — `unnecessary_ignore` through the engine
/// run's epilogue (#1345), `remove_deprecations_in_breaking_versions`
/// through the shell's late-join walk — and both now derive as emittable.
/// A further name here is a dead seam the re-freeze must reason about.
const STRUCTURALLY_SILENT: &[&str] = &[
    "analyzer_element_model_tracking_bad",
    "analyzer_element_model_tracking_more_than_one",
    "analyzer_element_model_tracking_zero",
    "analyzer_public_api_bad_part_directive",
    "analyzer_public_api_bad_type",
    "analyzer_public_api_experimental_inconsistency",
    "analyzer_public_api_exports_non_public_name",
    "analyzer_public_api_impl_in_public_api",
    "analyzer_to_be_deprecated_use",
];

/// The registered set rules the implementation can emit — the crate's
/// single derivation ([`emittable_rule_names`], every mount seam
/// included), collected by name for the pins below.
fn emittable_set_rules() -> BTreeSet<&'static str> {
    emittable_rule_names().collect()
}

/// The freeze file's code rows: one lowercase machine name per line, `#`
/// comments and blanks ignored (the gate script's own reading).
fn freeze_rows(text: &str) -> Vec<&str> {
    text.lines()
        .map(|line| line.trim_end_matches('\r'))
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect()
}

/// The `gate_status` tokens the ledger's column prose defines. The TSV
/// carries no header row (the columns live in its `#` comment block), so
/// the status column is located by this vocabulary, not by position.
const GATE_STATUSES: &[&str] = &["excluded", "in_universe", "fixed"];

/// The ledger's `gate_status=excluded` rule names: rules consciously out of
/// the gate universe. The ledger is the source of truth for gate
/// exclusions — a new `excluded` row carves the rule out of this pin (and
/// out of the freeze) automatically; there is no list here to keep in sync.
///
/// Row shape (the file's column prose): `rule`, `face`, `direction`,
/// `gate_status`, `citation`, tab-separated; `#` full-line comments, no
/// header row, the `face` prose never carries tabs. A data row without
/// exactly one status token fails loudly — format drift surfaces here as a
/// tutorial panic, never as a silent misparse.
fn ledger_excluded(text: &str) -> BTreeSet<String> {
    let mut excluded = BTreeSet::new();
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut fields = line.split('\t');
        let rule = fields.next().unwrap_or_default();
        let statuses: Vec<&str> = fields
            .filter(|field| GATE_STATUSES.contains(field))
            .collect();
        assert!(
            statuses.len() == 1,
            "{LINT_DIVERGENCES}: row {rule:?} carries {statuses:?} where exactly one \
             gate_status token belongs ({GATE_STATUSES:?}) — the ledger format drifted; \
             fix the row or this parser"
        );
        if statuses[0] == "excluded" {
            excluded.insert(rule.to_ascii_lowercase());
        }
    }
    excluded
}

/// The governance pin: the freeze must equal the registry-derived universe
/// minus the divergence ledger's `excluded` rows (the ledger is the source
/// of truth for gate exclusions — a new exclusion row carves the rule out
/// of this pin automatically). Catches the three drift directions that
/// matter — a retired/mis-spelled/silent row the gate would never see
/// fire, a mounted rule missing from both the freeze and the ledger, and
/// an excluded rule rotting back into the freeze.
#[test]
fn lint_codes_freeze_matches_the_registry() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(LINT_CODES_FREEZE);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
    let rows = freeze_rows(&text);

    let ledger_path = Path::new(env!("CARGO_MANIFEST_DIR")).join(LINT_DIVERGENCES);
    let ledger = std::fs::read_to_string(&ledger_path)
        .unwrap_or_else(|e| panic!("reading {}: {e}", ledger_path.display()));
    let excluded = ledger_excluded(&ledger);

    let mut seen = BTreeSet::new();
    let dupes: Vec<&str> = rows
        .iter()
        .copied()
        .filter(|row| !seen.insert(*row))
        .collect();
    assert!(
        dupes.is_empty(),
        "{LINT_CODES_FREEZE}: duplicate entries: {dupes:?}"
    );

    let emittable = emittable_set_rules();
    let mut orphans = Vec::new();
    for row in &rows {
        let Some(id) = rule_by_name(row) else {
            orphans.push(format!("{row}: not a registered rule"));
            continue;
        };
        let spec = rules()[id.0 as usize];
        if spec.group == Group::Diagnostic {
            orphans.push(format!("{row}: a diagnostic mirror, opt-in by name only"));
        } else if !emittable.contains(spec.name) {
            orphans.push(format!("{row}: structurally silent (kinds: &[])"));
        }
    }
    assert!(
        orphans.is_empty(),
        "{LINT_CODES_FREEZE}: rows the implementation cannot emit: {orphans:?}"
    );

    let frozen: BTreeSet<&str> = rows.iter().copied().collect();

    // The #1109 invariant: a ledger `excluded` row means the rule is out of
    // the gate universe — it must not sit in the freeze. This is the
    // carve-out's rot guard: a regen that re-adds excluded rows, or a
    // ledger row flipped without re-freezing, fails here instead of at
    // gate time.
    let rot: Vec<&String> = excluded
        .iter()
        .filter(|name| frozen.contains(name.as_str()))
        .collect();
    assert!(
        rot.is_empty(),
        "{LINT_CODES_FREEZE}: ledger-excluded rules still in the gate universe: {rot:?} — \
         an `excluded` row in {LINT_DIVERGENCES} rules the rule out of the gate; drop the \
         freeze rows, or retire the ledger row (in_universe/fixed) once the divergence \
         converged and re-freeze"
    );

    let gaps: Vec<String> = emittable
        .iter()
        .filter(|name| {
            let machine = name.to_ascii_lowercase();
            !frozen.contains(machine.as_str()) && !excluded.contains(machine.as_str())
        })
        .map(|name| name.to_ascii_lowercase())
        .collect();
    assert!(
        gaps.is_empty(),
        "{LINT_CODES_FREEZE}: mounted rules missing from the freeze and carrying no ledger \
         `excluded` row (the gate would never judge them): {gaps:?} — re-freeze via \
         regen_lint_codes_freeze and reconcile the header prose, or rule the divergence \
         in {LINT_DIVERGENCES}"
    );

    // The structurally silent set is the registered set rules minus the
    // emittable derivation (equivalently: empty `kinds` and no mount-face
    // seam membership — the seams live inside `emittable_rule_names`).
    let actually_silent: BTreeSet<&'static str> = rules()
        .iter()
        .filter(|spec| !matches!(spec.group, Group::Diagnostic | Group::Removed))
        .filter(|spec| !emittable.contains(spec.name))
        .map(|spec| spec.name)
        .collect();
    let silent: BTreeSet<&str> = STRUCTURALLY_SILENT.iter().copied().collect();
    assert_eq!(
        silent, actually_silent,
        "STRUCTURALLY_SILENT must name exactly the set rules that cannot emit \
         (registry holds {actually_silent:?}; the pin lists {silent:?})"
    );
}

/// Regenerates the freeze file's entry block from the registry-derived
/// universe, preserving the curated comment header verbatim. Oracle-free:
/// the inclusion rule is mechanical (the registered set rules that can
/// emit). Run explicitly, then reconcile the header prose with the new
/// rows — the re-freeze is deliberate, the regen is only its mechanical
/// half:
///
/// ```text
/// cargo test -p dartq-lints --test registry_pin regen_lint_codes_freeze -- --ignored
/// ```
#[test]
#[ignore = "rewrites tests/harness/lint_codes.txt; explicit re-freeze maintenance"]
fn regen_lint_codes_freeze() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(LINT_CODES_FREEZE);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
    let header_end = text
        .lines()
        .position(|line| {
            let line = line.trim_end_matches('\r');
            !line.is_empty() && !line.starts_with('#')
        })
        .expect("the freeze file carries an entry block");

    let mut entries: Vec<String> = emittable_set_rules()
        .into_iter()
        .map(str::to_ascii_lowercase)
        .collect();
    entries.sort();

    let old: BTreeSet<String> = freeze_rows(&text)
        .into_iter()
        .map(str::to_ascii_lowercase)
        .collect();
    let new: BTreeSet<String> = entries.iter().cloned().collect();
    let added: Vec<&String> = new.difference(&old).collect();
    let removed: Vec<&String> = old.difference(&new).collect();

    let mut out = String::with_capacity(text.len() + entries.len() * 24);
    for line in text.lines().take(header_end) {
        out.push_str(line.trim_end_matches('\r'));
        out.push('\n');
    }
    for entry in &entries {
        out.push_str(entry);
        out.push('\n');
    }
    std::fs::write(&path, out).unwrap_or_else(|e| panic!("writing {}: {e}", path.display()));

    println!(
        "regen_lint_codes_freeze: {} entries (was {}); reconcile the comment header",
        entries.len(),
        old.len()
    );
    println!("  added: {added:?}");
    println!("  removed: {removed:?}");
}
