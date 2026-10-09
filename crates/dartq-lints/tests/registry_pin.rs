//! Registry completeness pin: every wave-1 rule must exist in the official
//! `package:lints` sets, with its [`Group`] matching the set it came from.
//!
//! The lists below are pinned verbatim from `.third/lints/lib/core.yaml` and
//! `.third/lints/lib/recommended.yaml` (the pinned `package:lints` snapshot;
//! AGENTS.md fact-source discipline). When the snapshot re-pins, update the
//! lists here in the same change as the registry.

use dartq_lints::{rule_by_name, EnableSet, Group, RULES};

/// `.third/lints/lib/core.yaml` — the critical set, pinned verbatim.
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

/// `.third/lints/lib/recommended.yaml` — the idiom set (includes core via
/// `include:`), pinned verbatim.
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

#[test]
fn rule_names_are_unique() {
    let mut seen: Vec<&str> = Vec::with_capacity(RULES.len());
    for spec in RULES {
        assert!(
            seen.iter().all(|s| *s != spec.name),
            "duplicate rule name {}",
            spec.name
        );
        seen.push(spec.name);
    }
}

/// The completeness pin: a registry entry that leaves the official sets (or
/// carries the wrong `Group`) breaks the crate's contract.
#[test]
fn every_rule_is_in_its_official_set() {
    for spec in RULES {
        let official = match spec.group {
            Group::Core => OFFICIAL_CORE,
            Group::Recommended => OFFICIAL_RECOMMENDED,
        };
        assert!(
            official.contains(&spec.name),
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
    for spec in RULES {
        if spec.group == Group::Recommended {
            assert!(
                !OFFICIAL_CORE.contains(&spec.name),
                "{} is a core-set rule but registered as Recommended",
                spec.name
            );
        }
    }
}

/// [`EnableSet`] stores one 64-bit word; the registry must stay within it.
/// Grow `WORDS` together with this bound when the set outgrows a word.
#[test]
fn registry_fits_the_enable_set_word_count() {
    assert!(
        RULES.len() <= 64,
        "registry holds {} rules; EnableSet has a single 64-bit word",
        RULES.len()
    );
}

#[test]
fn rule_by_name_round_trips() {
    for (i, spec) in RULES.iter().enumerate() {
        let id = rule_by_name(spec.name).expect("registered name");
        assert_eq!(id.0 as usize, i, "{} sits at a stale index", spec.name);
    }
    assert_eq!(rule_by_name("no_such_lint_name"), None);
}

#[test]
fn enable_set_all_and_resolve_agree() {
    let all = EnableSet::all();
    for i in 0..RULES.len() {
        assert!(all.is_enabled(dartq_lints::RuleId(i as u16)));
    }
    let (resolved, unknown) = EnableSet::resolve(RULES.iter().map(|s| s.name));
    assert!(unknown.is_empty());
    assert_eq!(resolved, all);
}

/// Every template renders: problem texts are non-empty and `{n}` slots fill
/// (the two-variant rules pass their texts explicitly, so `report_texts`
/// covers them — this pins the registry-level templates only). Literal
/// braces in prose (e.g. `'{}'` in empty_constructor_bodies) are fine; only
/// unfilled `{n}` slots fail.
#[test]
fn templates_are_well_formed() {
    for spec in RULES {
        assert!(!spec.problem.is_empty(), "{} has no problemMessage", spec.name);
        let rendered = spec.render_problem(&["X"]);
        assert!(!rendered.contains("{0}"), "{} left a raw slot", spec.name);
        if !spec.correction.is_empty() {
            assert!(spec.render_correction(&["X"]).is_some());
        } else {
            assert_eq!(spec.render_correction(&[]), None);
        }
    }
}
