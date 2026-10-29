//! Corpus id-families that are deliberately not registry codes.
//!
//! The oracle SDK test corpus organizes cases under suite directories
//! whose names are not diagnostic codes or lint rules. A corpus family
//! census (2026-10-29) verified for every id below that its cases emit
//! real registry codes in their payload rows and never a row under the
//! family's own name:
//! - 112 diagnostics grouping labels (e.g. `constructor_body` covers the
//!   cases that emit `const_constructor_with_body` and friends),
//! - 3 lint suite groupings whose cases emit registry lint names
//!   (`analyzer_public_api` -> `analyzer_public_api_bad_type`, ...),
//! - 2 SDK harness buckets (`_core`, `_misc`).
//!
//! These ids get no status row by design and are excluded from the
//! unregistered-code warning, which therefore reports only genuine gaps.

/// Sorted id list; lookups use binary search.
pub const GROUPING_IDS: &[&str] = &[
    "_core",
    "_misc",
    "abi_specific_integer_mapping",
    "abstract_field_constructor_initializer",
    "ambiguous_set_or_map_literal",
    "analyzer_element_model_tracking",
    "analyzer_public_api",
    "annotation_syntax",
    "augmentation_formal_parameter",
    "avoid_field_initializers_in_non_const_classes",
    "base_class_implemented_outside_of_library",
    "base_mixin_implemented_outside_of_library",
    "built_in_identifier_as_extension_name",
    "built_in_identifier_as_extension_type_name",
    "built_in_identifier_as_prefix_name",
    "built_in_identifier_as_type_name",
    "built_in_identifier_as_type_parameter_name",
    "built_in_identifier_as_typedef_name",
    "case_block_not_terminated",
    "conflicting_constructor_and_static_field",
    "conflicting_constructor_and_static_method",
    "const_field_initializer_not_assignable",
    "const_variable_augmentation",
    "const_with_non_type",
    "constructor_body",
    "deprecated_extends_function",
    "deprecated_implements_function",
    "deprecated_mixin_function",
    "doc_directive_missing_one_argument",
    "doc_directive_missing_three_arguments",
    "doc_directive_missing_two_arguments",
    "duplicate_constructor_default",
    "duplicate_constructor_name",
    "duplicate_private_named_parameter",
    "executable_body",
    "extends_deferred_class",
    "extends_disallowed_class",
    "extends_type_alias_expands_to_type_parameter",
    "extension_declares_field",
    "extension_type_implements_not_supertype_of_erasure",
    "external_field_constructor_initializer",
    "ffi_address_of_cast",
    "ffi_array",
    "ffi_async_callback",
    "ffi_leaf_call_must_not_use_handle",
    "ffi_native",
    "final_class_extended_outside_of_library",
    "final_class_implemented_outside_of_library",
    "final_class_used_as_mixin_constraint_outside_of_library",
    "illegal_concrete_enum_member_declaration",
    "illegal_concrete_enum_member_inheritance",
    "illegal_enum_values_declaration",
    "illegal_enum_values_inheritance",
    "implements_deferred_class",
    "implements_disallowed_class",
    "implements_type_alias_expands_to_type_parameter",
    "initializing_declaring",
    "interface_class_extended_outside_of_library",
    "invalid_immutable_annotation",
    "invalid_language_override",
    "invalid_language_override_greater",
    "invalid_null_aware_elements_error",
    "invalid_type_argument_in_const_list",
    "invalid_type_argument_in_const_map",
    "invalid_type_argument_in_const_set",
    "mixin_class_declaration",
    "mixin_deferred_class",
    "mixin_inference_no_possible_substitution",
    "mixin_of_disallowed_class",
    "mixin_of_type_alias_expands_to_type_parameter",
    "mixin_on_type_alias_expands_to_type_parameter",
    "mixin_subtype_of_base_is_not_base",
    "mixin_subtype_of_final_is_not_base",
    "mixin_super_class_constraint_disallowed_class",
    "mixins_super_class",
    "new_with_non_type",
    "no_default_super_constructor_explicit",
    "non_constant_list_element_from_deferred_library",
    "non_constant_map_key_from_deferred_library",
    "non_constant_map_value_from_deferred_library",
    "non_exhaustive_switch",
    "null_aware_elements_const_literals_error",
    "null_safety_read_write",
    "number_literals_with_separators",
    "override_on_non_overriding_field",
    "override_on_non_overriding_getter",
    "override_on_non_overriding_method",
    "override_on_non_overriding_setter",
    "recursive_factory_redirect",
    "recursive_interface_inheritance_extends",
    "recursive_interface_inheritance_implements",
    "recursive_interface_inheritance_on",
    "recursive_interface_inheritance_with",
    "return_of_invalid_type_from_catch_error",
    "return_of_invalid_type_from_then",
    "return_type_invalid_for_catch_error",
    "return_type_invalid_for_then",
    "sealed_class_subtype_outside_of_library",
    "set_element_from_deferred_library",
    "subtype_of_base_is_not_base_final_or_sealed",
    "subtype_of_ffi_class",
    "subtype_of_final_is_not_base_final_or_sealed",
    "text_direction_code_point",
    "top_level_instance_getter",
    "type_check_is_not_null",
    "type_check_is_null",
    "undefined_class_boolean",
    "undefined_constructor_in_initializer_default",
    "undefined_enum_constructor_named",
    "undefined_enum_constructor_unnamed",
    "undefined_super_getter",
    "undefined_super_operator",
    "undefined_super_setter",
    "use_of_nullable_value",
    "use_of_private_parameter_name",
    "variable_not_initialized",
    "yield_each_in_non_generator",
];

/// Whether a registry-missing attributed id is an accounted exclusion.
pub fn is_excluded(id: &str) -> bool {
    GROUPING_IDS.binary_search(&id).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_is_sorted_and_deduplicated() {
        assert!(!GROUPING_IDS.is_empty());
        let mut sorted = GROUPING_IDS.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.as_slice(), GROUPING_IDS);
    }

    #[test]
    fn spots_groupings_harness_buckets_and_not_real_rules() {
        assert!(is_excluded("constructor_body"));
        assert!(is_excluded("_core"));
        assert!(is_excluded("_misc"));
        assert!(is_excluded("variable_not_initialized"));
        // Case-folded lint families resolve via the registry instead.
        assert!(!is_excluded("prefer_iterable_whereType"));
        assert!(!is_excluded("no_runtimeType_toString"));
        assert!(!is_excluded("prefer_for_elements_to_map_fromIterable"));
        assert!(!is_excluded("totally_unknown_code"));
    }
}
