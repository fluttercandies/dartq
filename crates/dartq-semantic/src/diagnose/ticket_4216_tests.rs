//! #4216 pins: the container type-parameter bound lookup resolves by
//! DECLARATION order, never hash order.
//!
//! Two containers in one file may share a parameter spelling with
//! different bounds (`class A<T extends num>` next to `class B<T extends
//! String>`). The post-walk name-only lookup used to scan
//! `FxHashMap<StringId, _>` captures and take the first match, so the
//! bound a reference resolved through depended on the map's iteration
//! order — which moves between runs because parallel scanning interns
//! names in a scheduler-dependent order: one reference read its own
//! container's bound, the next read the sibling's. The lookup now records
//! each header's name-token offset at capture and narrows to the nearest
//! declaring container at or before the reference, so each reference sees
//! its own scope.

use super::*;
use crate::collector::collect;
use crate::diagnose::{FileReport, StrictModes, diagnose_with};
use crate::library::ElementIndex;
use crate::model::MicroKind;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_resolution::{LibraryClassModifiers, LibraryGraphBuilder, Namespaces};
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

fn range(offset: u32, length: u32) -> SourceRange {
    SourceRange { offset, length }
}

/// The one-library rig: `source` is the sole analyzed file of
/// `package:t`, no imports.
fn unit_report(source: &str) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    let features = ParserFeatures::from_feature_set(set);
    let scanned = dartq_scanner::scan(
        source,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        file,
    );
    assert!(
        scanned.diagnostics.is_empty(),
        "scanner diagnostics: {source:?}"
    );
    let elements = collect(source, &scanned.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
    for (name, decl) in elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib, name, file);
    }
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let mut index = ElementIndex::default();
    index.insert(file, elements);
    diagnose_with(
        source,
        &scanned.tokens,
        features,
        &strings,
        file,
        lib,
        &graph,
        &namespaces,
        &index,
        StrictModes::default(),
        &LibraryClassModifiers::default(),
        set.is_enabled(dartq_language::Feature::InferenceUpdate2),
    )
}

/// The `value.count` selector offsets, in source order — the nullability
/// row anchors at the selector token, not the receiver.
fn member_offsets(source: &str) -> Vec<u32> {
    source
        .match_indices("value.count")
        .map(|(index, _)| u32::try_from(index + "value.".len()).expect("offset fits u32"))
        .collect()
}

/// The `unchecked_use_of_nullable_value` row offsets — the family the
/// receiver-bound nullability face drives (the #4202 signal).
fn nullable_rows(report: &FileReport) -> Vec<u32> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unchecked_use_of_nullable_value")
        .map(|d| d.range.offset)
        .collect()
}

/// Two containers share the parameter spelling `T`; the bounded one's
/// reference reads a non-nullable bound (clean), the unbounded one's the
/// implicit `Object?` (the row). Each reference must resolve through its
/// own container's bound, so exactly one row lands, at the unbounded
/// container's selector — whichever container the capture map happens to
/// iterate first.
#[test]
fn sibling_container_param_bounds_resolve_per_container() {
    let source = "\
class Base {
  int count = 0;
}

class A<T extends Base> {
  int read(T value) => value.count;
}

class B<T> {
  int read(T value) => value.count;
}
";
    let report = unit_report(source);
    let offsets = member_offsets(source);
    assert_eq!(
        nullable_rows(&report),
        vec![offsets[1]],
        "only B's reference reads the unbounded (nullable) bound: {source:?}"
    );
}

/// The reverse declaration order (the earlier boundary answered both
/// references before the fix): the offset narrowing, not a fixed
/// first-declaration pick, is what keeps both references honest.
#[test]
fn sibling_container_param_bounds_resolve_in_reverse_order() {
    let source = "\
class Base {
  int count = 0;
}

class B<T> {
  int read(T value) => value.count;
}

class A<T extends Base> {
  int read(T value) => value.count;
}
";
    let report = unit_report(source);
    let offsets = member_offsets(source);
    assert_eq!(
        nullable_rows(&report),
        vec![offsets[0]],
        "only B's reference reads the unbounded (nullable) bound: {source:?}"
    );
}

/// The resolution unit face: a reference inside each container answers
/// that container's bound, a reference before every declaration answers
/// the first declaration, and no offset answers the first declaration —
/// all hash-order free.
#[test]
fn container_param_bound_narrows_by_reference_offset() {
    let strings = StringTable::new();
    let (a, b, t, num, string) = (
        strings.intern("A"),
        strings.intern("B"),
        strings.intern("T"),
        strings.intern("num"),
        strings.intern("String"),
    );
    let mut bounds = Bounds::new();
    // `class A<T extends num> { ... }` — the name token sits at 6, the
    // reference the caller asks about at 30 (inside A).
    bounds.begin_type_variables();
    bounds.leaf(num, false, range(13, 3));
    bounds.end_type_variable(t, true);
    bounds.end_type_variables();
    bounds.note_container(Some(a), 6, 21, false, false);
    // `class B<T extends String> { ... }` — name token at 43, reference
    // at 100 (inside B).
    bounds.begin_type_variables();
    bounds.leaf(string, false, range(50, 6));
    bounds.end_type_variable(t, true);
    bounds.end_type_variables();
    bounds.note_container(Some(b), 43, 58, false, false);
    let bound_text = |at: Option<u32>| {
        bounds
            .container_param_bound(t, at)
            .flatten()
            .map(|tree| strings.resolve(tree.head).to_owned())
    };
    assert_eq!(bound_text(Some(30)).as_deref(), Some("num"));
    assert_eq!(bound_text(Some(100)).as_deref(), Some("String"));
    // No reference offset (and a reference before every declaration): the
    // first declaration answers, deterministically.
    assert_eq!(bound_text(None).as_deref(), Some("num"));
    assert_eq!(bound_text(Some(0)).as_deref(), Some("num"));
    // A spelling no container declares answers nothing.
    let unknown = strings.intern("U");
    assert!(
        bounds.container_param_bound(unknown, Some(100)).is_none(),
        "a spelling no own container declares has no bound"
    );
}
