//! Pin: a constructor's function-typed formal survives a type-parameter
//! bound that completes beside the annotation's own root (#3086).
//!
//! `class C<X extends B<X>> { C(bool Function()? f); }` — the class
//! header's bound argument group (`B<X>`) is captured in the same window
//! the constructor's formal annotation opens. The bound's completed root
//! can sit beside the sealed function shape, and a single-root window
//! (`done.len() == 1`) then refused to arm the shape: the formal kept the
//! `bool` return type the annotation had armed — a bare `Ty::Named` —
//! instead of the `bool Function()?` it spells.
//!
//! The close now takes the LAST root, the discipline `end_record_type`
//! already keeps for an extension header's type-parameter group: the
//! just-sealed shape is the last node the close pushed, so a leftover
//! root below it can never strand the function type.
//!
//! Harness: scanner + collector alone (the capture layer the rule-0c
//! formal substitution grounds against), reading the container's
//! `ctor_formals` — the unnamed face keys on the class name.

use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_scanner::{ScanConfig, scan};
use dartq_semantic::collector::collect;
use dartq_semantic::model::{FileElements, Ty};
use dartq_source::{FileTable, StringId, StringTable};
use std::path::Path;

/// Runs `source` through scanner + collector — the same posture the
/// collector unit tests use, minus diagnose (the pin targets the capture
/// layer alone).
fn build(source: &str, strings: &StringTable) -> FileElements {
    let mut files = FileTable::new();
    let file = files.intern(Path::new("test.dart"));
    let result = scan(
        source,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        file,
    );
    assert!(
        result.diagnostics.is_empty(),
        "scanner diagnostics: {:?}",
        result.diagnostics
    );
    let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(
        LanguageVersion::new(3, 15),
    ));
    collect(source, &result.tokens, features, strings)
}

fn id(strings: &StringTable, name: &str) -> StringId {
    strings.intern(name)
}

/// The unnamed constructor's positional formal at `at`, read through the
/// class-name key the call-site schema resolves.
fn ctor_formal_ty(
    elements: &FileElements,
    strings: &StringTable,
    container: &str,
    at: usize,
) -> Ty {
    elements
        .container(id(strings, container))
        .unwrap_or_else(|| panic!("container {container}"))
        .ctor_formals
        .get(&id(strings, container))
        .unwrap_or_else(|| panic!("unnamed ctor formal table for {container}"))
        .positional
        .get(at)
        .unwrap_or_else(|| panic!("positional formal {at} on {container}"))
        .ty
}

/// The named formal `name` of the unnamed constructor on `container`.
fn named_ctor_formal_ty(
    elements: &FileElements,
    strings: &StringTable,
    container: &str,
    name: &str,
) -> Ty {
    elements
        .container(id(strings, container))
        .unwrap_or_else(|| panic!("container {container}"))
        .ctor_formals
        .get(&id(strings, container))
        .unwrap_or_else(|| panic!("unnamed ctor formal table for {container}"))
        .named_formal(id(strings, name))
        .unwrap_or_else(|| panic!("named formal {name} on {container}"))
        .ty
}

/// The ticket's shape with a named return type: `bool Function()?` must
/// stay a first-class `Ty::Function` even though the class header's bound
/// completed its own root first. Degrades to a bare `bool` (`Ty::Named`)
/// under the single-root window.
#[test]
fn ctor_fn_typed_formal_survives_a_bounded_type_parameter() {
    let strings = StringTable::new();
    let elements = build(
        "class B<T> {}\n\
         class C<X extends B<X>> { C(bool Function()? f); }",
        &strings,
    );
    let ty = ctor_formal_ty(&elements, &strings, "C", 0);
    assert_eq!(
        ty.render(&strings, elements.type_args()),
        "bool Function()?",
        "the fn-typed formal keeps the written shape"
    );
}

/// The literal shape from the ticket (`void Function()?`): the `void`
/// return type arms `Ty::Void`, which the old window left on the formal.
#[test]
fn ctor_void_fn_typed_formal_survives_a_bounded_type_parameter() {
    let strings = StringTable::new();
    let elements = build(
        "class B<T> {}\n\
         class C<X extends B<X>> { C(void Function()? f); }",
        &strings,
    );
    let ty = ctor_formal_ty(&elements, &strings, "C", 0);
    assert_eq!(
        ty.render(&strings, elements.type_args()),
        "void Function()?"
    );
}

/// The bound's own arguments nest (`B<B<X>>`, closed by one compound
/// `>>>`): the split closer must not strand the shape either.
#[test]
fn ctor_fn_typed_formal_survives_a_nested_bound_argument() {
    let strings = StringTable::new();
    let elements = build(
        "class B<T> {}\n\
         class C<X extends B<B<X>>> { C(bool Function()? f); }",
        &strings,
    );
    let ty = ctor_formal_ty(&elements, &strings, "C", 0);
    assert_eq!(
        ty.render(&strings, elements.type_args()),
        "bool Function()?"
    );
}

/// A preceding ordinary formal keeps its own type and the follower keeps
/// the shape — the arm is per formal, never a shifted window.
#[test]
fn ctor_fn_typed_formal_follows_an_ordinary_formal() {
    let strings = StringTable::new();
    let elements = build(
        "class B<T> {}\n\
         class C<X extends B<X>> { C(int n, void Function()? f); }",
        &strings,
    );
    let render = |at: usize| {
        ctor_formal_ty(&elements, &strings, "C", at).render(&strings, elements.type_args())
    };
    assert_eq!(render(0), "int");
    assert_eq!(render(1), "void Function()?");
}

/// The legacy spelling (`bool f()` — the old-style function-typed formal)
/// folds through the same close, so it survives the bound identically.
#[test]
fn legacy_fn_typed_formal_survives_a_bounded_type_parameter() {
    let strings = StringTable::new();
    let elements = build(
        "class B<T> {}\n\
         class C<X extends B<X>> { C(bool f()); }",
        &strings,
    );
    let ty = ctor_formal_ty(&elements, &strings, "C", 0);
    assert_eq!(ty.render(&strings, elements.type_args()), "bool Function()");
}

/// The signature's own bounded group (`Function<Y extends B<Y>>`) rides
/// the shape verbatim beside the class's bound.
#[test]
fn generic_fn_typed_formal_keeps_its_own_bounded_group() {
    let strings = StringTable::new();
    let elements = build(
        "class B<T> {}\n\
         class C<X extends B<X>> { C(void Function<Y extends B<Y>>(Y) f); }",
        &strings,
    );
    let ty = ctor_formal_ty(&elements, &strings, "C", 0);
    assert_eq!(
        ty.render(&strings, elements.type_args()),
        "void Function<Y extends B<Y>>(Y)"
    );
}

/// The baseline the window must not regress: a non-generic class's
/// `Function()` formal interns its reserved shape unchanged.
#[test]
fn non_generic_ctor_fn_typed_formal_is_unchanged() {
    let strings = StringTable::new();
    let elements = build("class C { C(void Function()? f); }", &strings);
    let ty = ctor_formal_ty(&elements, &strings, "C", 0);
    assert_eq!(
        ty.render(&strings, elements.type_args()),
        "void Function()?"
    );
}

/// The named-formal sibling of the window: a default value's construction
/// (`const Foo<List<int>>()`) completes its own argument group inside the
/// formal window (the default-value arm suppresses `handle_type`, so that
/// root is never armed away). The follower function-typed formal must
/// still intern the written shape instead of inheriting the `bool` it
/// armed on the way in.
#[test]
fn fn_typed_formal_survives_a_preceding_default_value_type() {
    let strings = StringTable::new();
    let elements = build(
        "class Foo<T> { const Foo(); }\n\
         class C { C({bool a = const Foo<List<int>>(), void Function()? f}); }",
        &strings,
    );
    let ty = named_ctor_formal_ty(&elements, &strings, "C", "f");
    assert_eq!(
        ty.render(&strings, elements.type_args()),
        "void Function()?"
    );
}

/// The positional-optional spelling of the same window.
#[test]
fn fn_typed_formal_survives_a_preceding_default_value_in_positional_optional() {
    let strings = StringTable::new();
    let elements = build(
        "class Foo<T> { const Foo(); }\n\
         class C { C([bool a = const Foo<List<int>>(), bool Function(int)? f]); }",
        &strings,
    );
    let ty = elements
        .container(id(&strings, "C"))
        .expect("C")
        .ctor_formals
        .get(&id(&strings, "C"))
        .expect("unnamed ctor formal table")
        .positional
        .last()
        .expect("f")
        .ty;
    assert_eq!(
        ty.render(&strings, elements.type_args()),
        "bool Function(int)?"
    );
}

/// The stale root is drained at its source (#3097): a following
/// parameterized annotation in the same list keeps its argument list. An
/// undrained root made `complete_type_site` read a two-root window and drop
/// the capture, so `b` degraded to a bare `List`.
#[test]
fn default_value_construction_does_not_strand_the_next_annotation() {
    let strings = StringTable::new();
    let elements = build(
        "class Foo<T> { const Foo(); }\n\
         class C { C({bool a = const Foo<List<int>>(), List<int>? b}); }",
        &strings,
    );
    let ty = named_ctor_formal_ty(&elements, &strings, "C", "b");
    assert_eq!(ty.render(&strings, elements.type_args()), "List<int>?");
}

/// The same drain holds for a multi-argument following annotation, and the
/// default value's position (positional-optional) makes no difference.
#[test]
fn positional_default_value_does_not_strand_the_next_annotation() {
    let strings = StringTable::new();
    let elements = build(
        "class Foo<T> { const Foo(); }\n\
         class C { C([bool a = const Foo<List<int>>(), Map<String, int>? b]); }",
        &strings,
    );
    let ty = ctor_formal_ty(&elements, &strings, "C", 1);
    assert_eq!(
        ty.render(&strings, elements.type_args()),
        "Map<String, int>?"
    );
}

/// The bound itself round-trips on the container face: the header's
/// `X extends B<X>` keeps its name and its captured argument list, so the
/// close that arms the function shape did not consume the bound's root.
#[test]
fn bounded_type_parameter_round_trips() {
    let strings = StringTable::new();
    let elements = build(
        "class B<T> {}\n\
         class C<X extends B<X>> { C(bool Function()? f); }",
        &strings,
    );
    let container = elements.container(id(&strings, "C")).expect("C");
    assert_eq!(container.type_params, 1);
    let names: Vec<&str> = container
        .type_param_names
        .iter()
        .map(|name| strings.resolve(*name))
        .collect();
    assert_eq!(names, vec!["X"]);
    let bounds: Vec<String> = container
        .type_param_bounds
        .iter()
        .map(|bound| bound.render(&strings, elements.type_args()))
        .collect();
    assert_eq!(bounds, vec!["B<X>".to_string()]);
}
