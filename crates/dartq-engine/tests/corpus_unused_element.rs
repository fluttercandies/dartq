//! Corpus slice: `unused_element` — a declared element (class, method,
//! field, parameter, top-level declaration) that is never referenced.
//! RED per `conformance/status.tsv`; the slice pins the unused/lint-face
//! layer (`scopes_bodies.rs`) burned by ticket #4130 (W1 burn,
//! unused_finalization + scopes_bodies remainder).
//!
//! The family is a single-class monster (391 cases under one
//! `UnusedElementTest` class), so it is deep-sliced by id prefix chunks of
//! ≤60 cases each. Every prefix is covered exactly once (zero silent
//! drops): `classPrivate` (70) + `classPublic` (31) + the 290-case
//! remainder grouped by first-token prefix.

mod corpus_common;

// --- classPrivate constructors (70 cases) ---------------------------------

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_class_private_primary_constructor_named_private_slice() {
    corpus_common::run_family(
        "diagnostics/unused_element/UnusedElementTest/test_classPrivate_primaryConstructor_namedPrivate",
    );
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_class_private_primary_constructor_named_public_slice() {
    corpus_common::run_family(
        "diagnostics/unused_element/UnusedElementTest/test_classPrivate_primaryConstructor_namedPublic",
    );
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_class_private_primary_constructor_unnamed_slice() {
    corpus_common::run_family(
        "diagnostics/unused_element/UnusedElementTest/test_classPrivate_primaryConstructor_unnamed",
    );
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_class_private_secondary_constructor_named_private_slice() {
    corpus_common::run_family(
        "diagnostics/unused_element/UnusedElementTest/test_classPrivate_secondaryConstructor_namedPrivate",
    );
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_class_private_secondary_constructor_named_public_slice() {
    corpus_common::run_family(
        "diagnostics/unused_element/UnusedElementTest/test_classPrivate_secondaryConstructor_namedPublic",
    );
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_class_private_secondary_constructor_unnamed_slice() {
    corpus_common::run_family(
        "diagnostics/unused_element/UnusedElementTest/test_classPrivate_secondaryConstructor_unnamed",
    );
}

// --- classPublic constructors (31 cases) -----------------------------------

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_class_public_primary_constructor_named_public_slice() {
    corpus_common::run_family(
        "diagnostics/unused_element/UnusedElementTest/test_classPublic_primaryConstructor_namedPublic",
    );
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_class_public_primary_constructor_unnamed_slice() {
    corpus_common::run_family(
        "diagnostics/unused_element/UnusedElementTest/test_classPublic_primaryConstructor_unnamed",
    );
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_class_public_secondary_constructor_named_private_slice() {
    corpus_common::run_family(
        "diagnostics/unused_element/UnusedElementTest/test_classPublic_secondaryConstructor_namedPrivate",
    );
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_class_public_secondary_constructor_named_public_slice() {
    corpus_common::run_family(
        "diagnostics/unused_element/UnusedElementTest/test_classPublic_secondaryConstructor_namedPublic",
    );
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_class_public_secondary_constructor_unnamed_slice() {
    corpus_common::run_family(
        "diagnostics/unused_element/UnusedElementTest/test_classPublic_secondaryConstructor_unnamed",
    );
}

// --- method (47 cases) ------------------------------------------------------

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_method_is_used_slice() {
    corpus_common::run_family("diagnostics/unused_element/UnusedElementTest/test_method_isUsed");
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_method_not_used_slice() {
    corpus_common::run_family("diagnostics/unused_element/UnusedElementTest/test_method_notUsed");
}

// --- parameter (33 cases) ---------------------------------------------------

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_parameter_is_used_slice() {
    corpus_common::run_family("diagnostics/unused_element/UnusedElementTest/test_parameter_isUsed");
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_parameter_not_used_slice() {
    corpus_common::run_family(
        "diagnostics/unused_element/UnusedElementTest/test_parameter_notUsed",
    );
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_parameter_missing_name_slice() {
    corpus_common::run_family(
        "diagnostics/unused_element/UnusedElementTest/test_parameter_missingName",
    );
}

// --- dotShorthand (31 cases) -------------------------------------------------

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_dot_shorthand_parameter_slice() {
    corpus_common::run_family(
        "diagnostics/unused_element/UnusedElementTest/test_dotShorthand_parameter",
    );
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_dot_shorthand_private_slice() {
    corpus_common::run_family(
        "diagnostics/unused_element/UnusedElementTest/test_dotShorthand_private",
    );
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_dot_shorthand_public_slice() {
    corpus_common::run_family(
        "diagnostics/unused_element/UnusedElementTest/test_dotShorthand_public",
    );
}

// --- privateEnum (30 cases) --------------------------------------------------

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_private_enum_private_instance_method_slice() {
    corpus_common::run_family(
        "diagnostics/unused_element/UnusedElementTest/test_privateEnum_privateInstanceMethod",
    );
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_private_enum_private_instance_getter_slice() {
    corpus_common::run_family(
        "diagnostics/unused_element/UnusedElementTest/test_privateEnum_privateInstanceGetter",
    );
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_private_enum_private_instance_setter_slice() {
    corpus_common::run_family(
        "diagnostics/unused_element/UnusedElementTest/test_privateEnum_privateInstanceSetter",
    );
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_private_enum_private_static_method_slice() {
    corpus_common::run_family(
        "diagnostics/unused_element/UnusedElementTest/test_privateEnum_privateStaticMethod",
    );
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_private_enum_private_static_getter_slice() {
    corpus_common::run_family(
        "diagnostics/unused_element/UnusedElementTest/test_privateEnum_privateStaticGetter",
    );
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_private_enum_private_static_setter_slice() {
    corpus_common::run_family(
        "diagnostics/unused_element/UnusedElementTest/test_privateEnum_privateStaticSetter",
    );
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_private_enum_private_constructor_slice() {
    corpus_common::run_family(
        "diagnostics/unused_element/UnusedElementTest/test_privateEnum_privateConstructor",
    );
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_private_enum_public_constructor_slice() {
    corpus_common::run_family(
        "diagnostics/unused_element/UnusedElementTest/test_privateEnum_publicConstructor",
    );
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_private_enum_public_instance_method_slice() {
    corpus_common::run_family(
        "diagnostics/unused_element/UnusedElementTest/test_privateEnum_publicInstanceMethod",
    );
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_private_enum_public_instance_getter_slice() {
    corpus_common::run_family(
        "diagnostics/unused_element/UnusedElementTest/test_privateEnum_publicInstanceGetter",
    );
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_private_enum_public_instance_setter_slice() {
    corpus_common::run_family(
        "diagnostics/unused_element/UnusedElementTest/test_privateEnum_publicInstanceSetter",
    );
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_private_enum_public_static_method_slice() {
    corpus_common::run_family(
        "diagnostics/unused_element/UnusedElementTest/test_privateEnum_publicStaticMethod",
    );
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_private_enum_public_static_getter_slice() {
    corpus_common::run_family(
        "diagnostics/unused_element/UnusedElementTest/test_privateEnum_publicStaticGetter",
    );
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_private_enum_public_static_setter_slice() {
    corpus_common::run_family(
        "diagnostics/unused_element/UnusedElementTest/test_privateEnum_publicStaticSetter",
    );
}

// --- class_ (27 cases) --------------------------------------------------------

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_class_is_used_slice() {
    corpus_common::run_family("diagnostics/unused_element/UnusedElementTest/test_class_isUsed");
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_class_not_used_slice() {
    corpus_common::run_family("diagnostics/unused_element/UnusedElementTest/test_class_notUsed");
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_class_field_slice() {
    corpus_common::run_family("diagnostics/unused_element/UnusedElementTest/test_class_field");
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_class_getter_slice() {
    corpus_common::run_family("diagnostics/unused_element/UnusedElementTest/test_class_getter");
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_class_getter_setter_slice() {
    corpus_common::run_family(
        "diagnostics/unused_element/UnusedElementTest/test_class_getterSetter",
    );
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_class_setter_slice() {
    corpus_common::run_family("diagnostics/unused_element/UnusedElementTest/test_class_setter");
}

// --- publicEnum (15 cases) -----------------------------------------------------

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_public_enum_private_constructor_slice() {
    corpus_common::run_family(
        "diagnostics/unused_element/UnusedElementTest/test_publicEnum_privateConstructor",
    );
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_public_enum_public_constructor_slice() {
    corpus_common::run_family(
        "diagnostics/unused_element/UnusedElementTest/test_publicEnum_publicConstructor",
    );
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_public_enum_private_static_method_slice() {
    corpus_common::run_family(
        "diagnostics/unused_element/UnusedElementTest/test_publicEnum_privateStaticMethod",
    );
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_public_enum_private_static_getter_slice() {
    corpus_common::run_family(
        "diagnostics/unused_element/UnusedElementTest/test_publicEnum_privateStaticGetter",
    );
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_public_enum_private_static_setter_slice() {
    corpus_common::run_family(
        "diagnostics/unused_element/UnusedElementTest/test_publicEnum_privateStaticSetter",
    );
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_public_enum_public_static_method_slice() {
    corpus_common::run_family(
        "diagnostics/unused_element/UnusedElementTest/test_publicEnum_publicStaticMethod",
    );
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_public_enum_public_static_getter_slice() {
    corpus_common::run_family(
        "diagnostics/unused_element/UnusedElementTest/test_publicEnum_publicStaticGetter",
    );
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_public_enum_public_static_setter_slice() {
    corpus_common::run_family(
        "diagnostics/unused_element/UnusedElementTest/test_publicEnum_publicStaticSetter",
    );
}

// --- constructor (15 cases) ---------------------------------------------------

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_constructor_is_used_slice() {
    corpus_common::run_family(
        "diagnostics/unused_element/UnusedElementTest/test_constructor_isUsed",
    );
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_constructor_not_used_slice() {
    corpus_common::run_family(
        "diagnostics/unused_element/UnusedElementTest/test_constructor_notUsed",
    );
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_constructor_public_slice() {
    corpus_common::run_family(
        "diagnostics/unused_element/UnusedElementTest/test_constructorPublic",
    );
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_constructor_factory_slice() {
    corpus_common::run_family(
        "diagnostics/unused_element/UnusedElementTest/test_constructorFactory",
    );
}

// --- getter (10 cases) ---------------------------------------------------------

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_getter_is_used_slice() {
    corpus_common::run_family("diagnostics/unused_element/UnusedElementTest/test_getter_isUsed");
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_getter_not_used_slice() {
    corpus_common::run_family("diagnostics/unused_element/UnusedElementTest/test_getter_notUsed");
}

// --- typeAlias + functionTypeAlias (14 cases) -----------------------------------

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_type_alias_function_type_slice() {
    corpus_common::run_family(
        "diagnostics/unused_element/UnusedElementTest/test_typeAlias_functionType",
    );
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_type_alias_interface_type_slice() {
    corpus_common::run_family(
        "diagnostics/unused_element/UnusedElementTest/test_typeAlias_interfaceType",
    );
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_function_type_alias_slice() {
    corpus_common::run_family(
        "diagnostics/unused_element/UnusedElementTest/test_functionTypeAlias",
    );
}

// --- setter (7 cases) -------------------------------------------------------------

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_setter_is_used_slice() {
    corpus_common::run_family("diagnostics/unused_element/UnusedElementTest/test_setter_isUsed");
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_setter_not_used_slice() {
    corpus_common::run_family("diagnostics/unused_element/UnusedElementTest/test_setter_notUsed");
}

// --- extensionType + extensionTypePrivate + extension (14 cases) ------------------

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_extension_type_is_used_slice() {
    corpus_common::run_family(
        "diagnostics/unused_element/UnusedElementTest/test_extensionType_isUsed",
    );
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_extension_type_not_used_slice() {
    corpus_common::run_family(
        "diagnostics/unused_element/UnusedElementTest/test_extensionType_notUsed",
    );
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_extension_type_member_slice() {
    corpus_common::run_family(
        "diagnostics/unused_element/UnusedElementTest/test_extensionType_member",
    );
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_extension_type_private_constructor_slice() {
    corpus_common::run_family(
        "diagnostics/unused_element/UnusedElementTest/test_extensionType_privateConstructor",
    );
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_extension_type_private_slice() {
    corpus_common::run_family(
        "diagnostics/unused_element/UnusedElementTest/test_extensionTypePrivate",
    );
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_extension_unnamed_slice() {
    corpus_common::run_family("diagnostics/unused_element/UnusedElementTest/test_extension_");
}

// --- topLevel* (18 cases) -----------------------------------------------------------

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_top_level_function_slice() {
    corpus_common::run_family("diagnostics/unused_element/UnusedElementTest/test_topLevelFunction");
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_top_level_variable_slice() {
    corpus_common::run_family("diagnostics/unused_element/UnusedElementTest/test_topLevelVariable");
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_top_level_getter_setter_slice() {
    corpus_common::run_family(
        "diagnostics/unused_element/UnusedElementTest/test_topLevelGetterSetter",
    );
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_top_level_setter_slice() {
    corpus_common::run_family("diagnostics/unused_element/UnusedElementTest/test_topLevelSetter");
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_top_level_accessors_slice() {
    corpus_common::run_family(
        "diagnostics/unused_element/UnusedElementTest/test_topLevelAccessors",
    );
}

// --- publicStaticMethod (6 cases) ---------------------------------------------------

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_public_static_method_private_class_slice() {
    corpus_common::run_family(
        "diagnostics/unused_element/UnusedElementTest/test_publicStaticMethod_privateClass",
    );
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_public_static_method_private_extension_slice() {
    corpus_common::run_family(
        "diagnostics/unused_element/UnusedElementTest/test_publicStaticMethod_privateExtension",
    );
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_public_static_method_private_mixin_slice() {
    corpus_common::run_family(
        "diagnostics/unused_element/UnusedElementTest/test_publicStaticMethod_privateMixin",
    );
}

// --- localFunction + functionLocal + function (11 cases) -----------------------------

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_local_function_slice() {
    corpus_common::run_family("diagnostics/unused_element/UnusedElementTest/test_localFunction");
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_function_local_slice() {
    corpus_common::run_family("diagnostics/unused_element/UnusedElementTest/test_functionLocal");
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_function_underscore_slice() {
    corpus_common::run_family("diagnostics/unused_element/UnusedElementTest/test_function_");
}

// --- enum (6 cases) --------------------------------------------------------------------

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_enum_constructor_parameter_slice() {
    corpus_common::run_family("diagnostics/unused_element/UnusedElementTest/test_enum_constructor");
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_enum_is_used_slice() {
    corpus_common::run_family("diagnostics/unused_element/UnusedElementTest/test_enum_isUsed");
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_enum_not_used_slice() {
    corpus_common::run_family("diagnostics/unused_element/UnusedElementTest/test_enum_notUsed");
}

// --- mixin + publicTopLevelFunction + fieldImplicitGetter (4 cases) ---------------------

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_mixin_slice() {
    corpus_common::run_family("diagnostics/unused_element/UnusedElementTest/test_mixin");
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_public_top_level_function_slice() {
    corpus_common::run_family(
        "diagnostics/unused_element/UnusedElementTest/test_publicTopLevelFunction",
    );
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_field_implicit_getter_slice() {
    corpus_common::run_family(
        "diagnostics/unused_element/UnusedElementTest/test_fieldImplicitGetter",
    );
}

// --- doc case (1 case) -------------------------------------------------------------------

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_element_doc_slice() {
    corpus_common::run_family("diagnostics/unused_element/doc_");
}
