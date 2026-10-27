//! The published analyzer/parser documentation table: which official
//! non-lint codes carry a `https://dart.dev/diagnostics/<code>` page,
//! the one fact the JSON face's `documentation` field keys on for
//! analyzer rows.
//!
//! Upstream truth (`DiagnosticCode.url`, upstream Dart SDK source,
//! `pkg/_fe_analyzer_shared/lib/src/base/errors.dart`): the URL is
//! `https://dart.dev/diagnostics/<lowercase name>` when the code's
//! registry entry is `hasPublishedDocs: true`, `null` otherwise —
//! unlike lint codes, every one of which carries a page
//! (`crate::lint_docs`). The published set is captured from the
//! analyzer's consolidated generated registry (upstream Dart SDK
//! source, `pkg/analyzer/lib/src/diagnostic.g.dart`, dart 3.15 main
//! pin — generated from the analyzer/front_end/_fe_analyzer_shared
//! `messages.yaml` sources, with the camelCase entries' final snake
//! names already resolved): 582 of the 982 registry names, no LINT
//! rows among them (lints never enter this registry), and no name
//! with mixed published/unpublished variants — so the name-keyed
//! any-published fold is exact.
//!
//! The JSON face decides by code name here, never by the row's
//! `has_published_docs` flag: the flag is persisted per row (the
//! content store's `code_published`) and would serve stale URLs from
//! pre-capture stores, and emitters build `CodeInfo` rows
//! independently of this crate — the name-keyed table is stable
//! across warm and cold rows alike (same posture as `lint_docs`).
//! `dartq/`-namespace codes never spell a table name (snake_case
//! without `/`), so they take no URL by construction.
//!
//! Version-pinned data like `crate::code`'s generated table: sorted
//! by name, membership read by one binary search.

/// The analyzer/parser codes whose upstream registry entry is
/// `hasPublishedDocs: true` — each maps to
/// `https://dart.dev/diagnostics/<name>`.
static ANALYZER_PUBLISHED_DOCS: &[&str] = &[
    "abi_specific_integer_invalid", "abi_specific_integer_mapping_extra", "abi_specific_integer_mapping_missing",
    "abi_specific_integer_mapping_unsupported", "abstract_field_initializer", "abstract_sealed_class",
    "abstract_super_member_reference", "address_position", "address_receiver",
    "ambiguous_export", "ambiguous_extension_member_access", "ambiguous_import",
    "ambiguous_set_or_map_literal_both", "ambiguous_set_or_map_literal_either", "analysis_option_deprecated",
    "annotation_on_pointer_field", "argument_must_be_a_constant", "argument_must_be_native",
    "argument_type_not_assignable", "argument_type_not_assignable_to_error_handler", "assert_in_redirecting_constructor",
    "asset_directory_does_not_exist", "asset_does_not_exist", "asset_field_not_list",
    "asset_missing_path", "asset_not_string", "asset_not_string_or_map",
    "asset_path_not_string", "assignment_of_do_not_store", "assignment_to_const",
    "assignment_to_final", "assignment_to_final_local", "assignment_to_final_no_setter",
    "assignment_to_function", "assignment_to_method", "assignment_to_type",
    "async_for_in_wrong_context", "await_in_late_local_variable_initializer", "await_of_incompatible_type",
    "body_might_complete_normally", "body_might_complete_normally_catch_error", "body_might_complete_normally_nullable",
    "break_label_on_switch_member", "built_in_identifier_as_type", "built_in_identifier_in_declaration",
    "case_expression_type_implements_equals", "case_expression_type_is_not_switch_expression_subtype", "cast_from_null_always_fails",
    "cast_from_nullable_always_fails", "cast_to_non_type", "class_used_as_mixin",
    "class_used_as_mixin_declares_generative_constructor", "collection_element_from_deferred_library", "compound_implements_finalizable",
    "concrete_class_has_enum_superinterface", "concrete_class_with_abstract_member", "conflicting_constructor_and_static_member",
    "conflicting_generic_interfaces", "conflicting_type_variable_and_container", "conflicting_type_variable_and_member",
    "const_constructor_param_type_mismatch", "const_constructor_with_field_initialized_by_non_const", "const_constructor_with_non_const_super",
    "const_constructor_with_non_final_field", "const_deferred_class", "const_initialized_with_non_constant_value",
    "const_initialized_with_non_constant_value_from_deferred_library", "const_instance_field", "const_map_key_not_primitive_equality",
    "const_not_initialized", "const_set_element_not_primitive_equality", "const_spread_expected_list_or_set",
    "const_spread_expected_map", "const_with_non_const", "const_with_non_constant_argument",
    "const_with_type_parameters", "constant_pattern_never_matches_value_type", "constant_pattern_with_non_constant_expression",
    "continue_label_invalid", "creation_of_struct_or_union", "creation_with_non_type",
    "dead_code", "dead_code_catch_following_catch", "dead_code_on_catch_subtype",
    "dead_null_aware_expression", "default_value_in_function_type", "default_value_in_redirecting_factory_constructor",
    "default_value_on_required_parameter", "deferred_import_of_extension", "definitely_unassigned_late_local_variable",
    "dependencies_field_not_map", "deprecated_colon_for_default_value", "deprecated_export_use",
    "deprecated_extend", "deprecated_factory_method", "deprecated_field",
    "deprecated_implement", "deprecated_instantiate", "deprecated_lint",
    "deprecated_member_use", "deprecated_mixin", "deprecated_optional",
    "deprecated_subclass", "deprecated_subtype_of_function", "disallowed_type_instantiation_expression",
    "doc_import_cannot_be_deferred", "doc_import_cannot_have_configurations", "dot_shorthand_missing_context",
    "dot_shorthand_undefined_member", "duplicate_constructor", "duplicate_definition",
    "duplicate_export", "duplicate_field_formal_parameter", "duplicate_field_name",
    "duplicate_hidden_name", "duplicate_ignore", "duplicate_import",
    "duplicate_named_argument", "duplicate_part", "duplicate_pattern_assignment_variable",
    "duplicate_pattern_field", "duplicate_rest_element_in_pattern", "duplicate_rule",
    "duplicate_shown_name", "duplicate_variable_pattern", "empty_map_pattern",
    "empty_record_literal_with_comma", "empty_record_type_named_fields_list", "empty_record_type_with_comma",
    "empty_struct", "enum_constant_same_name_as_enclosing", "enum_mixin_with_instance_variable",
    "enum_with_abstract_member", "enum_with_name_values", "equal_elements_in_const_set",
    "equal_elements_in_set", "equal_keys_in_const_map", "equal_keys_in_map",
    "equal_keys_in_map_pattern", "expected_one_list_pattern_type_arguments", "expected_one_list_type_arguments",
    "expected_one_set_type_arguments", "expected_two_map_pattern_type_arguments", "expected_two_map_type_arguments",
    "export_internal_library", "export_of_non_library", "expression_in_map",
    "extends_non_class", "extension_as_expression", "extension_conflicting_static_and_instance",
    "extension_declares_abstract_member", "extension_declares_constructor", "extension_declares_instance_field",
    "extension_declares_member_of_object", "extension_override_access_to_static_member", "extension_override_argument_not_assignable",
    "extension_override_with_cascade", "extension_override_without_access", "extension_type_constructor_with_super_formal_parameter",
    "extension_type_constructor_with_super_invocation", "extension_type_declares_instance_field", "extension_type_declares_member_of_object",
    "extension_type_implements_disallowed_type", "extension_type_implements_itself", "extension_type_implements_not_supertype",
    "extension_type_implements_representation_not_supertype", "extension_type_inherited_member_conflict", "extension_type_representation_depends_on_itself",
    "extension_type_representation_type_bottom", "extension_type_with_abstract_member", "external_with_initializer",
    "extra_annotation_on_struct_field", "extra_positional_arguments", "extra_positional_arguments_could_be_named",
    "extra_size_annotation_carray", "ffi_native_invalid_duplicate_default_asset", "ffi_native_invalid_multiple_annotations",
    "ffi_native_must_be_external", "ffi_native_only_classes_extending_nativefieldwrapperclass1_can_be_pointer", "ffi_native_unexpected_number_of_parameters",
    "ffi_native_unexpected_number_of_parameters_with_receiver", "field_initialized_by_multiple_initializers", "field_initialized_in_initializer_and_declaration",
    "field_initialized_in_parameter_and_initializer", "field_initializer_factory_constructor", "field_initializer_not_assignable",
    "field_initializer_outside_constructor", "field_initializer_redirecting_constructor", "field_initializing_formal_not_assignable",
    "field_must_be_external_in_struct", "final_initialized_in_declaration_and_constructor", "final_not_initialized",
    "final_not_initialized_constructor", "flutter_field_not_map", "for_in_of_invalid_element_type",
    "for_in_of_invalid_type", "for_in_with_const_variable", "generic_method_type_instantiation_on_dynamic",
    "generic_struct_subclass", "getter_not_subtype_setter_types", "illegal_async_generator_return_type",
    "illegal_async_return_type", "illegal_concrete_enum_member", "illegal_enum_values",
    "illegal_sync_generator_return_type", "implements_non_class", "implements_repeated",
    "implements_super_class", "implicit_super_initializer_missing_arguments", "implicit_this_reference_in_initializer",
    "import_deferred_library_with_load_function", "import_internal_library", "import_of_non_library",
    "include_file_not_found", "included_file_parse_error", "included_file_warning",
    "incompatible_lint", "inconsistent_inheritance", "inconsistent_language_version_override",
    "inconsistent_pattern_variable_logical_or", "inference_failure_on_collection_literal", "inference_failure_on_function_invocation",
    "inference_failure_on_function_return_type", "inference_failure_on_uninitialized_variable", "inference_failure_on_untyped_parameter",
    "initializer_for_non_existent_field", "initializer_for_static_field", "initializing_formal_for_non_existent_field",
    "instance_access_to_static_member", "instance_member_access_from_factory", "instance_member_access_from_static",
    "instantiate_abstract_class", "instantiate_enum", "instantiate_type_alias_expands_to_type_parameter",
    "integer_literal_imprecise_as_double", "integer_literal_out_of_range", "invalid_annotation",
    "invalid_annotation_constant_value_from_deferred_library", "invalid_annotation_from_deferred_library", "invalid_annotation_target",
    "invalid_assignment", "invalid_await_not_required_annotation", "invalid_dependency",
    "invalid_deprecated_extend_annotation", "invalid_deprecated_implement_annotation", "invalid_deprecated_instantiate_annotation",
    "invalid_deprecated_mixin_annotation", "invalid_deprecated_optional_annotation", "invalid_deprecated_subclass_annotation",
    "invalid_exception_value", "invalid_export_of_internal_element", "invalid_export_of_internal_element_indirectly",
    "invalid_extension_argument_count", "invalid_factory_method_decl", "invalid_factory_method_impl",
    "invalid_factory_name_not_a_class", "invalid_field_name", "invalid_field_type_in_struct",
    "invalid_implementation_override", "invalid_inline_function_type", "invalid_internal_annotation",
    "invalid_language_version_override", "invalid_literal_annotation", "invalid_modifier_on_constructor",
    "invalid_modifier_on_setter", "invalid_non_virtual_annotation", "invalid_null_aware_operator",
    "invalid_option", "invalid_override", "invalid_override_of_non_virtual_member",
    "invalid_pattern_variable_in_shared_case_scope", "invalid_platforms_field", "invalid_reference_to_generative_enum_constructor",
    "invalid_reference_to_this", "invalid_reopen_annotation", "invalid_return_type_for_catch_error",
    "invalid_return_type_for_then", "invalid_section_format", "invalid_super_formal_parameter_location",
    "invalid_type_argument_in_const_literal", "invalid_uri", "invalid_use_of_covariant_in_extension",
    "invalid_use_of_do_not_submit_member", "invalid_use_of_internal_member", "invalid_use_of_null_value",
    "invalid_use_of_protected_member", "invalid_use_of_type_outside_library", "invalid_use_of_visible_for_overriding_member",
    "invalid_use_of_visible_for_testing_member", "invalid_visibility_annotation", "invalid_visible_outside_template_annotation",
    "invalid_widget_preview_application", "invalid_widget_preview_private_argument", "invocation_of_extension_without_call",
    "invocation_of_non_function", "invocation_of_non_function_expression", "label_in_outer_scope",
    "label_undefined", "late_final_field_with_const_constructor", "late_final_local_already_assigned",
    "leaf_call_must_not_return_handle", "leaf_call_must_not_take_handle", "list_element_type_not_assignable",
    "main_first_positional_parameter_type", "main_has_required_named_parameters", "main_has_too_many_required_positional_parameters",
    "main_is_not_function", "map_entry_not_in_map", "map_key_type_not_assignable",
    "map_value_type_not_assignable", "mismatched_annotation_on_struct_field", "missing_annotation_on_struct_field",
    "missing_default_value_for_parameter", "missing_dependency", "missing_enum_constant_in_switch",
    "missing_exception_value", "missing_field_type_in_struct", "missing_name",
    "missing_named_pattern_field_name", "missing_override_of_must_be_overridden", "missing_required_argument",
    "missing_required_param", "missing_size_annotation_carray", "missing_variable_pattern",
    "mixin_application_concrete_super_invoked_member_type", "mixin_application_no_concrete_super_invoked_member", "mixin_application_not_implemented_interface",
    "mixin_class_declaration_extends_not_object", "mixin_class_declaration_with_clause", "mixin_class_declares_non_trivial_generative_constructor",
    "mixin_inherits_from_not_object", "mixin_instantiate", "mixin_modifier_mixin_application_class_with_multiple_mixins",
    "mixin_of_non_class", "mixin_on_sealed_class", "mixin_super_class_constraint_deferred_class",
    "mixin_super_class_constraint_non_interface", "multiple_combinators", "multiple_redirecting_constructor_invocations",
    "multiple_super_initializers", "must_be_a_native_function_type", "must_be_a_subtype",
    "must_be_immutable", "must_call_super", "must_return_void",
    "name_not_string", "native_field_invalid_type", "native_field_missing_type",
    "native_field_not_static", "native_function_missing_type", "negative_variable_dimension",
    "new_with_undefined_constructor_default", "no_annotation_constructor_arguments", "no_combined_super_signature",
    "no_generative_constructors_in_superclass", "non_abstract_class_inherits_abstract_member", "non_bool_condition",
    "non_bool_expression", "non_bool_negation_expression", "non_bool_operand",
    "non_const_argument_for_const_parameter", "non_const_call_to_literal_constructor", "non_const_generative_enum_constructor",
    "non_constant_case_expression", "non_constant_case_expression_from_deferred_library", "non_constant_default_value",
    "non_constant_default_value_from_deferred_library", "non_constant_list_element", "non_constant_map_element",
    "non_constant_map_key", "non_constant_map_pattern_key", "non_constant_map_value",
    "non_constant_relational_pattern_expression", "non_constant_set_element", "non_constant_type_argument",
    "non_covariant_type_parameter_position_in_representation_type", "non_exhaustive_switch_expression", "non_exhaustive_switch_statement",
    "non_final_field_in_enum", "non_generative_constructor", "non_generative_implicit_constructor",
    "non_native_function_type_argument_to_pointer", "non_nullable_equals_parameter", "non_positive_array_dimension",
    "non_sized_type_argument", "non_sync_factory", "non_type_as_type_argument",
    "non_type_in_catch_clause", "non_void_return_for_operator", "non_void_return_for_setter",
    "not_a_type", "not_assigned_potentially_non_nullable_local_variable", "not_binary_operator",
    "not_enough_positional_arguments", "not_initialized_non_nullable_instance_field", "not_initialized_non_nullable_variable",
    "not_iterable_spread", "not_map_spread", "null_argument_to_non_null_type",
    "null_check_always_fails", "nullable_type_in_catch_clause", "nullable_type_in_extends_clause",
    "nullable_type_in_implements_clause", "nullable_type_in_on_clause", "nullable_type_in_with_clause",
    "obsolete_colon_for_default_value", "on_repeated", "optional_parameter_in_operator",
    "override_on_non_overriding_member", "packed_annotation", "packed_annotation_alignment",
    "parse_error", "part_of_different_library", "part_of_non_part",
    "part_of_unnamed_library", "path_does_not_exist", "path_not_posix",
    "path_pubspec_does_not_exist", "pattern_assignment_not_local_variable", "pattern_constant_from_deferred_library",
    "pattern_never_matches_value_type", "pattern_type_mismatch_in_irrefutable_context", "pattern_variable_assignment_inside_guard",
    "platform_value_disallowed", "positional_field_in_object_pattern", "positional_super_formal_parameter_with_positional_argument",
    "prefix_collides_with_top_level_member", "prefix_identifier_not_followed_by_dot", "prefix_shadowed_by_local_declaration",
    "primary_constructor_cannot_redirect", "private_collision_in_mixin_application", "private_named_non_field_parameter",
    "private_named_parameter_duplicate_public_name", "private_named_parameter_without_public_name", "private_optional_parameter",
    "private_setter", "read_potentially_unassigned_final", "record_literal_one_positional_no_trailing_comma",
    "record_type_one_positional_no_trailing_comma", "recursive_compile_time_constant", "recursive_constructor_redirect",
    "recursive_include_file", "recursive_interface_inheritance", "redeclare_on_non_redeclaring_member",
    "redirect_generative_to_missing_constructor", "redirect_generative_to_non_generative_constructor", "redirect_to_abstract_class_constructor",
    "redirect_to_invalid_function_type", "redirect_to_invalid_return_type", "redirect_to_missing_constructor",
    "redirect_to_non_class", "redirect_to_non_const_constructor", "redirect_to_type_alias_expands_to_type_parameter",
    "referenced_before_declaration", "refutable_pattern_in_irrefutable_context", "relational_pattern_operand_type_not_assignable",
    "relational_pattern_operator_return_type_not_assignable_to_bool", "removed_lint", "removed_lint_use",
    "rest_element_in_map_pattern", "rethrow_outside_catch", "return_in_generative_constructor",
    "return_in_generator", "return_of_do_not_store", "return_of_invalid_type",
    "return_of_invalid_type_from_closure", "return_without_value", "sdk_version_constructor_tearoffs",
    "sdk_version_gt_gt_gt_operator", "set_element_type_not_assignable", "shared_deferred_prefix",
    "size_annotation_dimensions", "static_access_to_instance_member", "subtype_of_base_or_final_is_not_base_final_or_sealed",
    "subtype_of_deferred_class", "subtype_of_disallowed_type", "subtype_of_sealed_class",
    "subtype_of_struct_class", "super_formal_parameter_type_is_not_subtype_of_associated", "super_formal_parameter_without_associated_named",
    "super_formal_parameter_without_associated_positional", "super_in_enum_constructor", "super_in_extension",
    "super_in_extension_type", "super_in_invalid_context", "super_in_redirecting_constructor",
    "super_invocation_not_last", "supertype_expands_to_type_parameter", "switch_case_completes_normally",
    "tearoff_of_generative_constructor_of_abstract_class", "text_direction_code_point_in_comment", "text_direction_code_point_in_literal",
    "throw_of_invalid_type", "top_level_cycle", "type_alias_cannot_reference_itself",
    "type_annotation_deferred_class", "type_argument_not_matching_bounds", "type_check_with_null",
    "type_parameter_referenced_by_static", "type_parameter_supertype_of_its_bound", "type_test_with_non_type",
    "type_test_with_undefined_name", "unawaited_return_in_try_block", "unchecked_use_of_nullable_value",
    "undefined_class", "undefined_constructor_in_initializer", "undefined_enum_constant",
    "undefined_enum_constructor", "undefined_extension_getter", "undefined_extension_method",
    "undefined_extension_operator", "undefined_extension_setter", "undefined_function",
    "undefined_getter", "undefined_hidden_name", "undefined_identifier",
    "undefined_identifier_await", "undefined_lint", "undefined_method",
    "undefined_named_parameter", "undefined_operator", "undefined_prefixed_name",
    "undefined_referenced_parameter", "undefined_setter", "undefined_shown_name",
    "undefined_super_member", "unknown_platform", "unnecessary_cast",
    "unnecessary_cast_pattern", "unnecessary_dev_dependency", "unnecessary_final",
    "unnecessary_import", "unnecessary_nan_comparison", "unnecessary_no_such_method",
    "unnecessary_non_null_assertion", "unnecessary_null_assert_pattern", "unnecessary_null_check_pattern",
    "unnecessary_null_comparison", "unnecessary_question_mark", "unnecessary_set_literal",
    "unnecessary_type_check", "unnecessary_wildcard_pattern", "unqualified_reference_to_non_local_static_member",
    "unqualified_reference_to_static_member_of_extended_type", "unreachable_switch_case", "unreachable_switch_default",
    "unrecognized_error_code", "unsupported_option", "unsupported_value",
    "unused_catch_clause", "unused_catch_stack", "unused_element",
    "unused_element_parameter", "unused_field", "unused_field_from_primary_constructor",
    "unused_import", "unused_label", "unused_local_variable",
    "unused_result", "unused_shown_name", "uri_does_not_exist",
    "uri_does_not_exist_in_doc_import", "uri_has_not_been_generated", "uri_with_interpolation",
    "use_of_native_extension", "use_of_void_result", "values_declaration_in_enum",
    "variable_length_array_not_last", "variable_pattern_keyword_in_declaration_context", "variable_type_mismatch",
    "workspace_field_not_list", "workspace_value_not_string", "workspace_value_not_subdirectory",
    "wrong_number_of_parameters_for_operator", "wrong_number_of_parameters_for_setter", "wrong_number_of_type_arguments",
    "wrong_number_of_type_arguments_constructor", "wrong_number_of_type_arguments_element", "wrong_number_of_type_arguments_enum",
    "wrong_number_of_type_arguments_extension", "yield_in_non_generator", "yield_of_invalid_type",
];

/// Whether `name` is an official analyzer/parser code with a
/// published docs page (the JSON `documentation` URL's gate for
/// non-lint rows).
#[must_use]
pub(crate) fn published(name: &str) -> bool {
    ANALYZER_PUBLISHED_DOCS.binary_search(&name).is_ok()
}

#[cfg(test)]
mod tests {
    use super::{ANALYZER_PUBLISHED_DOCS, published};

    /// The capture invariants: sorted and unique (binary search's
    /// contract), the universe size, and every name lowercase
    /// snake_case (the URL is spelled from the name verbatim — a
    /// casing drift would mint a dead URL and break the `dartq/`
    /// namespace separation argument in the module docs).
    #[test]
    fn table_is_sorted_unique_snake_case() {
        assert!(
            ANALYZER_PUBLISHED_DOCS.windows(2).all(|pair| pair[0] < pair[1]),
            "sorted and unique: binary search's contract"
        );
        assert_eq!(ANALYZER_PUBLISHED_DOCS.len(), 582);
        for name in ANALYZER_PUBLISHED_DOCS {
            assert!(
                name.chars()
                    .enumerate()
                    .all(|(i, c)| c.is_ascii_lowercase()
                        || c.is_ascii_digit() && i > 0
                        || c == '_' && i > 0),
                "{name}: not lowercase snake_case"
            );
        }
    }

    /// The pinned membership decisions — the ticket's regression row
    /// and the four edges the registry tests in `code.rs` pin per-row:
    /// published codes answer `true`, the unpublished parser pair and
    /// a dartq-own namespace name answer `false`.
    #[test]
    fn published_decides_by_table_membership() {
        for name in [
            "built_in_identifier_in_declaration",
            "uri_does_not_exist",
            "deprecated_member_use",
            "expected_token",
            "illegal_language_version_override",
        ] {
            // `expected_token` and `illegal_language_version_override`
            // are the two negative pins; the rest published.
            let expect = !matches!(
                name,
                "expected_token" | "illegal_language_version_override"
            );
            assert_eq!(published(name), expect, "{name}");
        }
        assert!(!published("dartq/internal_error"));
        assert!(!published("never_heard_of_it"));
    }
}
