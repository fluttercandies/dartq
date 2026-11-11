//! Capture-layer pins for #3847 (the args-less declared-face family):
//! the supertype clauses' written type arguments thread from the header
//! tree capture into the element model's parallel clause-args columns
//! (`Container::with_clause_args` and friends), the persisted payload
//! (`#[serde(default)]`, restore pads short columns), and the API
//! signature (v14).
//!
//! Upstream keeps the argument-bearing clause annotations on the element
//! (`endNamedMixinApplication`, upstream Dart SDK source,
//! pkg/analyzer/lib/src/fasta/ast_builder.dart:2499 — the with-clause
//! `NamedType`s carry their `typeArguments` into the element, where the
//! mixin application instantiates through them); these pins hold the
//! element face here to the same facts. Detector pins over the
//! substitution cascade live in `ticket_3847_tests.rs`.

use crate::collector::collect;
use crate::model::{ArgListId, Container, ContainerKind, Ty};
use crate::persisted::PersistedElements;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_scanner::ScanConfig;
use dartq_source::{FileTable, StringId, StringTable};
use std::path::Path;

/// Scans + collects one unit (the collector test posture).
fn elements_of(source: &str, strings: &StringTable) -> crate::model::FileElements {
    let mut files = FileTable::new();
    let file = files.intern(Path::new("ticket_3847u.dart"));
    let result = dartq_scanner::scan(
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
    collect(
        source,
        &result.tokens,
        ParserFeatures::from_feature_set(FeatureSet::for_language_version(LanguageVersion::new(
            3, 15,
        ))),
        strings,
    )
}

fn id(strings: &StringTable, name: &str) -> StringId {
    strings.intern(name)
}

/// A bare named leaf `T` (no written arguments, non-nullable).
fn leaf(strings: &StringTable, name: &str) -> Ty {
    Ty::Named {
        name: id(strings, name),
        nullable: false,
        args: ArgListId::EMPTY,
    }
}

/// The container row a name declares.
fn container_of<'a>(
    elements: &'a crate::model::FileElements,
    strings: &StringTable,
    name: &str,
) -> &'a Container {
    elements.container(id(strings, name)).unwrap_or_else(|| {
        let rows: Vec<String> = elements
            .container_order
            .iter()
            .map(|&row| strings.resolve(row).to_string())
            .collect();
        panic!("no container {name}; rows: {rows:?}")
    })
}

/// The interned argument list rendered as `Ty`s (a comparison face).
fn args_of(elements: &crate::model::FileElements, list: ArgListId) -> Vec<Ty> {
    elements.type_args.get(list).to_vec()
}

/// A named mixin application's `with` clause keeps each face's written
/// type arguments, parallel to the name column (`class C1<V> = Object
/// with M<V>` keeps `[V]` — the substitution env a receiver's `C1<int>`
/// zips onto `M`'s parameter).
#[test]
fn mixin_application_with_clause_args_capture() {
    let strings = StringTable::new();
    let elements = elements_of(
        "mixin M<T> {}\nclass A<U> {}\nclass C1<V> = Object with M<V>;",
        &strings,
    );
    let c1 = container_of(&elements, &strings, "C1");
    assert!(c1.is_mixin_application);
    assert_eq!(c1.kind, ContainerKind::Class);
    assert_eq!(c1.with_clause.as_ref(), &[id(&strings, "M")]);
    assert_eq!(c1.with_clause_args.len(), 1);
    assert_eq!(
        args_of(&elements, c1.with_clause_args[0]),
        [leaf(&strings, "V")]
    );
    // The head type is `Object` with no written arguments.
    assert_eq!(c1.superclass, Some(id(&strings, "Object")));
    assert_eq!(args_of(&elements, c1.superclass_args), [] as [Ty; 0]);
}

/// A multi-entry `with` list keeps every face's arguments in written
/// order (`with M1<T>, M2<int>` keeps `[[T], [int]]`).
#[test]
fn multi_entry_with_clause_args_capture() {
    let strings = StringTable::new();
    let elements = elements_of(
        "mixin M1<T> {}\nmixin M2<U> {}\nclass D<T> = Object with M1<T>, M2<int>;",
        &strings,
    );
    let d = container_of(&elements, &strings, "D");
    assert_eq!(
        d.with_clause.as_ref(),
        &[id(&strings, "M1"), id(&strings, "M2")]
    );
    assert_eq!(
        args_of(&elements, d.with_clause_args[0]),
        [leaf(&strings, "T")]
    );
    assert_eq!(
        args_of(&elements, d.with_clause_args[1]),
        [leaf(&strings, "int")]
    );
}

/// The `extends` head keeps its written type arguments beside the
/// superclass name (`extends S<int>` keeps `[int]`; a bare head keeps the
/// empty list).
#[test]
fn extends_head_args_capture() {
    let strings = StringTable::new();
    let elements = elements_of(
        "class S<X> {}\nclass E extends S<int> {}\nclass F extends S {}",
        &strings,
    );
    let e = container_of(&elements, &strings, "E");
    assert_eq!(e.superclass, Some(id(&strings, "S")));
    assert_eq!(
        args_of(&elements, e.superclass_args),
        [leaf(&strings, "int")]
    );
    let f = container_of(&elements, &strings, "F");
    assert_eq!(f.superclass, Some(id(&strings, "S")));
    assert_eq!(args_of(&elements, f.superclass_args), [] as [Ty; 0]);
}

/// A mixin's `on` constraint and `implements` edges keep their written
/// arguments, including nested parameterized arguments
/// (`on S<List<T>>` keeps `[List<T>]`).
#[test]
fn mixin_on_and_implements_args_capture() {
    let strings = StringTable::new();
    let mut elements = elements_of(
        "class S<X> {}\nclass I<Y> {}\nmixin N<T> on S<List<T>> implements I<int> {}",
        &strings,
    );
    let list_t = elements
        .type_args
        .named(id(&strings, "List"), false, &[leaf(&strings, "T")]);
    let n = container_of(&elements, &strings, "N");
    assert_eq!(n.on_clause.as_ref(), &[id(&strings, "S")]);
    assert_eq!(args_of(&elements, n.on_clause_args[0]), [list_t]);
    assert_eq!(n.implements_clause.as_ref(), &[id(&strings, "I")]);
    assert_eq!(
        args_of(&elements, n.implements_clause_args[0]),
        [leaf(&strings, "int")]
    );
}

/// An argument-less clause face carries the empty list, never a stale or
/// neighboring face's arguments.
#[test]
fn argless_clauses_stay_empty() {
    let strings = StringTable::new();
    let elements = elements_of(
        "class S<X> {}\nmixin M<T> {}\nclass I<Y> {}\nclass G extends S with M implements I {}",
        &strings,
    );
    let g = container_of(&elements, &strings, "G");
    assert_eq!(g.with_clause_args.len(), 1);
    assert_eq!(args_of(&elements, g.with_clause_args[0]), [] as [Ty; 0]);
    assert_eq!(g.implements_clause_args.len(), 1);
    assert_eq!(
        args_of(&elements, g.implements_clause_args[0]),
        [] as [Ty; 0]
    );
    assert_eq!(args_of(&elements, g.superclass_args), [] as [Ty; 0]);
}

/// A qualified clause face (`with p.M<int>`) keeps the written arguments
/// under the continuation name the name column carries — the tree pairs
/// by span containment (the annotation anchors at the prefix token and
/// still covers the continuation identifier).
#[test]
fn qualified_with_clause_face_args_capture() {
    let strings = StringTable::new();
    let elements = elements_of("class H = Object with p.M<int>;", &strings);
    let h = container_of(&elements, &strings, "H");
    assert_eq!(h.with_clause.as_ref(), &[id(&strings, "M")]);
    assert_eq!(
        args_of(&elements, h.with_clause_args[0]),
        [leaf(&strings, "int")]
    );
}

/// The declared member return face keeps ITS written arguments (the
/// fail-safe of this family: the substitution cascade's starved inputs
/// were never the member's own return — `A<V>` arrives with `[V]`; only
/// the clause faces arrived stripped to names).
#[test]
fn getter_return_keeps_written_args() {
    let strings = StringTable::new();
    let mut elements = elements_of(
        "class A<U> {}\nclass Box<V> {\n  A<V> get value => throw 0;\n}",
        &strings,
    );
    let expected = elements
        .type_args
        .named(id(&strings, "A"), false, &[leaf(&strings, "V")]);
    let box_row = container_of(&elements, &strings, "Box");
    let value = box_row.members.get(&id(&strings, "value")).unwrap();
    assert_eq!(value.return_ty, expected);
}

/// The augment fold merges the clause-args columns under the same rules
/// the names get: a fragment's new face joins with its own arguments, and
/// the introduction's face keeps the introduction's arguments.
#[test]
fn augment_fold_merges_clause_args() {
    let strings = StringTable::new();
    let elements = elements_of(
        "mixin M<T> {}\nclass C {}\naugment class C with M<int> {}",
        &strings,
    );
    let c = container_of(&elements, &strings, "C");
    assert_eq!(c.with_clause.as_ref(), &[id(&strings, "M")]);
    assert_eq!(
        args_of(&elements, c.with_clause_args[0]),
        [leaf(&strings, "int")]
    );

    let strings = StringTable::new();
    let elements = elements_of(
        "mixin M<T> {}\nclass C with M<int> {}\naugment class C with M {}",
        &strings,
    );
    let c = container_of(&elements, &strings, "C");
    assert_eq!(c.with_clause.as_ref(), &[id(&strings, "M")]);
    assert_eq!(
        args_of(&elements, c.with_clause_args[0]),
        [leaf(&strings, "int")]
    );
}

/// A clause-argument-only edit flips the API signature (v14): same clause
/// names, different written arguments — the L2 gate must not skip the
/// dependent cascade.
#[test]
fn clause_argument_only_edit_flips_signature() {
    let with_int = {
        let strings = StringTable::new();
        elements_of("mixin M<T> {}\nclass C = Object with M<int>;", &strings)
            .api_signature(&strings)
    };
    let with_string = {
        let strings = StringTable::new();
        elements_of("mixin M<T> {}\nclass C = Object with M<String>;", &strings)
            .api_signature(&strings)
    };
    let with_none = {
        let strings = StringTable::new();
        elements_of("mixin M<T> {}\nclass C = Object with M;", &strings).api_signature(&strings)
    };
    assert_ne!(with_int, with_string);
    assert_ne!(with_int, with_none);
    assert_ne!(with_string, with_none);
}

/// The persisted payload round-trips the clause-args columns, and a
/// pre-#3847 payload (the columns absent — the `#[serde(default)]`
/// additive posture) restores them padded to the name columns' length
/// with the empty list, never a short column.
#[test]
fn persisted_round_trip_keeps_clause_args() {
    let strings = StringTable::new();
    let elements = elements_of(
        "class S<X> {}\nmixin M<T> {}\nclass C<X> = S<List<X>> with M<int>;",
        &strings,
    );
    let payload = PersistedElements::of(&elements, &strings);
    let encoded = serde_json::to_string(&payload).expect("encode");
    let decoded: PersistedElements = serde_json::from_str(&encoded).expect("decode");
    let mut restored = decoded.into_elements(&strings);
    let list_x = restored
        .type_args
        .named(id(&strings, "List"), false, &[leaf(&strings, "X")]);
    let c = container_of(&restored, &strings, "C");
    assert_eq!(c.superclass, Some(id(&strings, "S")));
    assert_eq!(args_of(&restored, c.superclass_args), [list_x]);
    assert_eq!(c.with_clause.as_ref(), &[id(&strings, "M")]);
    assert_eq!(
        args_of(&restored, c.with_clause_args[0]),
        [leaf(&strings, "int")]
    );

    // The additive-default posture: strip the new columns from the payload
    // and decode — the restore pads to the names' length with EMPTY.
    let mut value: serde_json::Value = serde_json::from_str(&encoded).expect("json");
    let containers = value
        .get_mut("containers")
        .and_then(|c| c.as_array_mut())
        .expect("containers");
    for row in containers.iter_mut() {
        let face = row.get_mut(1).expect("container face");
        for column in [
            "with_clause_args",
            "implements_clause_args",
            "on_clause_args",
            "superclass_args",
        ] {
            face.as_object_mut().expect("face object").remove(column);
        }
    }
    let stripped = serde_json::to_string(&value).expect("re-encode");
    let legacy: PersistedElements = serde_json::from_str(&stripped).expect("legacy decode");
    let legacy_restored = legacy.into_elements(&strings);
    let c = container_of(&legacy_restored, &strings, "C");
    assert_eq!(c.with_clause.len(), 1);
    assert_eq!(
        args_of(&legacy_restored, c.with_clause_args[0]),
        [] as [Ty; 0]
    );
    assert_eq!(args_of(&legacy_restored, c.superclass_args), [] as [Ty; 0]);
}
