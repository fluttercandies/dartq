//! Pin: a constructor's function-typed formal carries the written shape
//! (#1550 — the issue63883 capture-layer face).
//!
//! `class C<X> { C(void Function(X) x); }` — the modern `R Function(P)`
//! spelling inside a constructor's formal list — interns through
//! `end_function_type`'s arm into a first-class `Ty::Function`, never
//! `Unknown` (the rule-0c poison the pre-#1482 capture applied). The solve
//! core grounds `X` against an argument's own function shape
//! (`(List<int> x) {}` unifies `X := List<int>`), so the row needs the
//! declared shape — return type, type-actor list, parameter list — on the
//! formal's `ty` slot.
//!
//! The pins read `Container::ctor_formals` under the unnamed face's
//! class-name key, mirroring the lookup spelling `ctor_schema_formals`
//! resolves at the call site.

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

/// The ctor formal's declared type, keyed under the container's own name
/// (the unnamed face's `ctor_formals` spelling).
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

/// The issue63883 shape verbatim: `C(void Function(X) x)` — a plain
// (non-generic) function-typed ctor formal whose parameter leaf names the
// class's own type parameter. `X` stays a bare named leaf inside the
// shape — the call-site substitution binds it.
#[test]
fn ctor_function_typed_formal_carries_the_written_shape() {
    let strings = StringTable::new();
    let elements = build("class C<X> { C(void Function(X) x); }", &strings);
    let ty = ctor_formal_ty(&elements, &strings, "C", 0);
    let rendered = ty.render(&strings, elements.type_args());
    assert_eq!(rendered, "void Function(X)");
}

/// A generic function-typed ctor formal (`C(void Function<Y>(Y) x)`): the
// signature's own `<Y>` group rides the shape's type-actor column — the
// written names intern beside the shape (the display's verbatim group),
// and `Y` inside the formal's own columns binds against it, never the
// class's parameters.
#[test]
fn ctor_generic_function_typed_formal_carries_the_written_shape() {
    let strings = StringTable::new();
    let elements = build("class C<X> { C(void Function<Y>(Y) x); }", &strings);
    let ty = ctor_formal_ty(&elements, &strings, "C", 0);
    let rendered = ty.render(&strings, elements.type_args());
    assert_eq!(rendered, "void Function<Y>(Y)");
}

/// A named constructor's fn-typed formal captures identically — the table
// keys on the selector, the shape discipline is the same arm.
#[test]
fn named_ctor_function_typed_formal_carries_the_written_shape() {
    let strings = StringTable::new();
    let elements = build("class C<X> { C.named(void Function(X) x); }", &strings);
    let ty = elements
        .container(id(&strings, "C"))
        .expect("C")
        .ctor_formals
        .get(&id(&strings, "named"))
        .expect("named selector table")
        .positional[0]
        .ty;
    assert_eq!(ty.render(&strings, elements.type_args()), "void Function(X)");
}
