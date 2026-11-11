//! Capture-layer pins for #3903 (M11 variance): a type parameter's variance
//! modifier (`in`/`out`/`inout`) threads from the parser's
//! `end_type_variable` event into the element model's
//! `Container::type_param_variances` column, through the persisted payload
//! (`#[serde(default)]`, restore pads short columns with the legacy code),
//! and into the API signature (v15).
//!
//! Upstream keeps the keyword beside the bound (`endTypeVariable`'s
//! `varianceKeyword`, upstream Dart SDK source,
//! pkg/analyzer/lib/src/fasta/ast_builder.dart) and reads it back per
//! interface argument (`_interfaceArguments`, pkg/analyzer/lib/src/dart/
//! element/subtype.dart): covariant checks `sub <: super`, contravariant the
//! reverse, invariant both, and a modifier-less parameter answers the
//! covariant relation (`TypeParameterElementImpl.variance`). Only class /
//! enum / mixin headers accept the keywords (the parser's `allowsVariance`
//! face), so the column rides the container row alone. Detector pins over
//! the subtype face belong to the consumer lanes; these pins hold the
//! capture to the written facts.

use crate::collector::collect;
use crate::model::{Container, FileElements};
use crate::persisted::PersistedElements;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_scanner::ScanConfig;
use dartq_source::{FileTable, StringId, StringTable};
use dartq_types::Variance;
use std::path::Path;

/// Scans + collects one unit (the collector test posture).
fn elements_of(source: &str, strings: &StringTable) -> FileElements {
    let mut files = FileTable::new();
    let file = files.intern(Path::new("ticket_3903.dart"));
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

/// The container row a name declares.
fn container_of<'a>(
    elements: &'a FileElements,
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

/// Each modifier spelling lands on its own reading, in declaration order:
/// `out` → covariant, `in` → contravariant, `inout` → invariant, and a
/// modifier-less parameter keeps the legacy covariant default.
#[test]
fn class_header_variance_modifiers_capture() {
    let strings = StringTable::new();
    let elements = elements_of(
        "class OutP<out T> {}\n\
         class InP<in T> {}\n\
         class InoutP<inout T> {}\n\
         class Legacy<T> {}\n\
         class Mixed<in K, V, out P, inout Q, R> {}",
        &strings,
    );
    let out_p = container_of(&elements, &strings, "OutP");
    assert_eq!(
        out_p.type_param_variances.as_ref(),
        [Some(Variance::Covariant)]
    );
    let in_p = container_of(&elements, &strings, "InP");
    assert_eq!(
        in_p.type_param_variances.as_ref(),
        [Some(Variance::Contravariant)]
    );
    let inout_p = container_of(&elements, &strings, "InoutP");
    assert_eq!(
        inout_p.type_param_variances.as_ref(),
        [Some(Variance::Invariant)]
    );
    let legacy = container_of(&elements, &strings, "Legacy");
    assert_eq!(legacy.type_param_variances.as_ref(), [None]);
    let mixed = container_of(&elements, &strings, "Mixed");
    assert_eq!(
        mixed.type_param_variances.as_ref(),
        [
            Some(Variance::Contravariant),
            None,
            Some(Variance::Covariant),
            Some(Variance::Invariant),
            None,
        ]
    );
}

/// A mixin header accepts the modifiers on the same face (the parser opens
/// every class-family declaration header with `allowsVariance`); the column
/// stays parallel to the written spellings.
#[test]
fn mixin_header_variance_modifiers_capture() {
    let strings = StringTable::new();
    let elements = elements_of("mixin M<inout T, U> {}\n", &strings);
    let m = container_of(&elements, &strings, "M");
    assert_eq!(m.type_param_names.len(), 2);
    assert_eq!(
        m.type_param_variances.as_ref(),
        [Some(Variance::Invariant), None]
    );
}

/// The modifier write pairs with the bound write on the same slot: the
/// group completes in reverse declaration order, so a name-keyed row is the
/// only pairing that keeps `in T extends num` as one parameter's facts.
#[test]
fn variance_and_bound_land_on_the_same_slot() {
    let strings = StringTable::new();
    let elements = elements_of(
        "class C<in T extends num, out U> {}\n\
         class D<inout A extends List<B>, out B> {}",
        &strings,
    );
    let c = container_of(&elements, &strings, "C");
    assert_eq!(
        c.type_param_names.as_ref(),
        &[id(&strings, "T"), id(&strings, "U")]
    );
    assert_eq!(
        c.type_param_variances.as_ref(),
        [Some(Variance::Contravariant), Some(Variance::Covariant)]
    );
    assert!(!matches!(c.type_param_bounds[0], crate::model::Ty::Dynamic));
    assert!(matches!(c.type_param_bounds[1], crate::model::Ty::Dynamic));

    let d = container_of(&elements, &strings, "D");
    assert_eq!(
        d.type_param_names.as_ref(),
        &[id(&strings, "A"), id(&strings, "B")]
    );
    assert_eq!(
        d.type_param_variances.as_ref(),
        [Some(Variance::Invariant), Some(Variance::Covariant)]
    );
    assert!(!matches!(d.type_param_bounds[0], crate::model::Ty::Dynamic));
    assert!(matches!(d.type_param_bounds[1], crate::model::Ty::Dynamic));
}

/// A nested `<...>` group inside one parameter's bound must not clobber the
/// enclosing group's modifiers: the outer reading survives the inner
/// quantifier (`#3748`'s frame discipline, extended to the modifier
/// buffer).
#[test]
fn nested_group_restore_keeps_outer_modifiers() {
    let strings = StringTable::new();
    let elements = elements_of(
        "class C<inout X extends void Function<T extends num>(T), out Y> {}\n",
        &strings,
    );
    let c = container_of(&elements, &strings, "C");
    assert_eq!(
        c.type_param_names.as_ref(),
        &[id(&strings, "X"), id(&strings, "Y")]
    );
    assert_eq!(
        c.type_param_variances.as_ref(),
        [Some(Variance::Invariant), Some(Variance::Covariant)]
    );
}

/// The modifiers survive the persisted payload round trip beside their
/// spellings and bounds.
#[test]
fn persisted_round_trip_keeps_variances() {
    let strings = StringTable::new();
    let elements = elements_of("class C<inout K extends num, out V, W> {}\n", &strings);
    let payload = PersistedElements::of(&elements, &strings);
    let encoded = serde_json::to_string(&payload).expect("encode");
    let decoded: PersistedElements = serde_json::from_str(&encoded).expect("decode");
    let restored = decoded.into_elements(&strings);
    let c = container_of(&restored, &strings, "C");
    assert_eq!(c.type_param_names.len(), 3);
    assert_eq!(
        c.type_param_variances.as_ref(),
        [Some(Variance::Invariant), Some(Variance::Covariant), None]
    );
    assert_eq!(c.type_param_bounds.len(), 3);
    assert!(!matches!(c.type_param_bounds[0], crate::model::Ty::Dynamic));
}

/// The additive-default posture: a pre-#3903 payload (the column absent)
/// decodes with the names intact and the modifier column padded back to
/// their length with the legacy code — never a short parallel row.
#[test]
fn legacy_payload_without_the_variance_column_decodes_as_legacy() {
    let strings = StringTable::new();
    let elements = elements_of("class C<inout K, out V> {}\n", &strings);
    let payload = PersistedElements::of(&elements, &strings);
    let encoded = serde_json::to_string(&payload).expect("encode");
    let mut value: serde_json::Value = serde_json::from_str(&encoded).expect("json");
    let containers = value
        .get_mut("containers")
        .and_then(|c| c.as_array_mut())
        .expect("containers");
    for row in containers.iter_mut() {
        let face = row.get_mut(1).expect("container face");
        face.as_object_mut()
            .expect("face object")
            .remove("type_param_variances");
    }
    let stripped = serde_json::to_string(&value).expect("re-encode");
    let legacy: PersistedElements = serde_json::from_str(&stripped).expect("legacy decode");
    let legacy_restored = legacy.into_elements(&strings);
    let c = container_of(&legacy_restored, &strings, "C");
    assert_eq!(c.type_param_names.len(), 2);
    assert_eq!(c.type_param_variances.as_ref(), [None, None]);
}

/// A modifier-only edit flips the API signature (v15): same parameter
/// names, different written modifiers — the L2 gate must not skip the
/// dependent cascade, because the interface-argument relation changes.
#[test]
fn modifier_only_edit_flips_the_signature() {
    let signature = |source: &str| {
        let strings = StringTable::new();
        elements_of(source, &strings).api_signature(&strings)
    };
    let legacy = signature("class C<T> {}\n");
    let covariant = signature("class C<out T> {}\n");
    let contravariant = signature("class C<in T> {}\n");
    let invariant = signature("class C<inout T> {}\n");
    assert_ne!(legacy, invariant);
    assert_ne!(legacy, contravariant);
    assert_ne!(legacy, covariant);
    assert_ne!(covariant, contravariant);
    assert_ne!(invariant, contravariant);
    assert_ne!(covariant, invariant);
}

/// The wire codes are the shared lattice's discriminants (`dartq_types`
/// keeps `meet` load-bearing on them) with `0` = no modifier, and an
/// unknown code (a payload this build does not write) reads back as the
/// no-modifier default — never an invented rejection.
#[test]
fn variance_wire_codes_encode_the_lattice_and_unknown_codes_fall_back() {
    let strings = StringTable::new();
    let elements = elements_of("class C<inout K, out V, W> {}\n", &strings);
    let payload = PersistedElements::of(&elements, &strings);
    let encoded = serde_json::to_string(&payload).expect("encode");
    let value: serde_json::Value = serde_json::from_str(&encoded).expect("json");
    let containers = value
        .get("containers")
        .and_then(|c| c.as_array())
        .expect("containers");
    let face = containers[0].get(1).expect("container face");
    let codes: Vec<u64> = face
        .get("type_param_variances")
        .and_then(|column| column.as_array())
        .expect("variance column")
        .iter()
        .map(|code| code.as_u64().expect("numeric code"))
        .collect();
    assert_eq!(codes, [3, 1, 0]);

    // An unknown code decodes as no modifier.
    let mut value = value;
    let containers = value
        .get_mut("containers")
        .and_then(|c| c.as_array_mut())
        .expect("containers");
    containers[0]
        .get_mut(1)
        .expect("container face")
        .as_object_mut()
        .expect("face object")
        .insert(
            "type_param_variances".to_owned(),
            serde_json::json!([9, 9, 9]),
        );
    let stripped = serde_json::to_string(&value).expect("re-encode");
    let decoded: PersistedElements = serde_json::from_str(&stripped).expect("decode");
    let restored = decoded.into_elements(&strings);
    let c = container_of(&restored, &strings, "C");
    assert_eq!(c.type_param_variances.as_ref(), [None, None, None]);
}
