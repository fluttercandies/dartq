//! #3418 pins — an augmented element's member face is the union of its
//! fragments', whichever unit of the library the spelling is written in.
//!
//! # The defect
//!
//! `resolve_container` was file-local first with no cross-fragment union, so
//! a body inside `augment class C { … }` saw only the augment's own members
//! (the #3407 shell row) and a body in the introducing unit saw only the
//! introduction's: `this.introMember` from an augment body and
//! `this.augmentMember` from the introducing body both read as undefined.
//! Upstream an `augment` head declares a FRAGMENT of the element its
//! introduction declares (`element.firstFragment` is the introduction,
//! upstream Dart SDK source, pkg/_fe_analyzer_shared augmentation handling),
//! so the element carries one member face — the union.
//!
//! # The pins
//!
//! ① the model face: `resolve_container` answers the union from either
//! anchor; ② an introduction member read inside an augment body; ③ an
//! augment-declared member read from the introducing body; ④ a member
//! declared in NEITHER still reports; ⑤ the merge is order-independent —
//! the same fragments in a different unit order answer the same face.

use super::test_rig::{code_names, report_ops_augment, report_ops_augment_intro_experiments, rows};
use crate::collector::collect;
use crate::library::{ElementIndex, resolve_container};
use crate::model::MicroKind;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_resolution::{LibraryGraph, LibraryGraphBuilder, LibraryId, Namespaces};
use dartq_scanner::ScanConfig;
use dartq_source::{FileId, FileTable, StringTable};
use std::path::Path;

/// One library of `units` (`path`, `source`) in the given unit order — the
/// order `graph.unit_files` answers, and the only thing the merge may not
/// depend on.
struct Fixture {
    strings: StringTable,
    graph: LibraryGraph,
    namespaces: Namespaces,
    index: ElementIndex,
    lib: LibraryId,
    files: Vec<FileId>,
}

fn library(units: &[(&str, &str)]) -> Fixture {
    let strings = StringTable::new();
    let mut table = FileTable::new();
    let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(
        LanguageVersion::new(3, 15),
    ));
    let mut models = Vec::with_capacity(units.len());
    for &(path, source) in units {
        let file = table.intern(Path::new(path));
        let scanned = dartq_scanner::scan(
            source,
            ScanConfig {
                enable_triple_shift: true,
                enable_augmentations: true,
            },
            file,
        );
        assert!(scanned.diagnostics.is_empty(), "{source:?}");
        let tokens: &'static dartq_scanner::TokenTable = Box::leak(Box::new(scanned.tokens));
        models.push((file, collect(source, tokens, features, &strings)));
    }
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
    let mut index = ElementIndex::default();
    let mut files = Vec::with_capacity(models.len());
    for (file, elements) in &models {
        index.insert(*file, elements.clone());
        builder.unit(lib, *file);
        for (name, decl) in elements.declarations() {
            if decl.kind == MicroKind::TypeParam {
                continue;
            }
            builder.declare(lib, name, *file);
        }
        files.push(*file);
    }
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    Fixture {
        strings,
        graph,
        namespaces,
        index,
        lib,
        files,
    }
}

impl Fixture {
    /// The member spellings the container anchor answers in `file`, sorted.
    fn members(&self, file: FileId, container: &str) -> Vec<String> {
        let name = self.strings.intern(container);
        let (face, owner) = resolve_container(
            &self.graph,
            &self.namespaces,
            &self.index,
            self.lib,
            file,
            name,
        )
        .unwrap_or_else(|| panic!("`{container}` resolves from the fixture's unit"));
        assert_eq!(owner, file, "the anchor owns the row it answers with");
        let mut members: Vec<String> = face
            .members
            .keys()
            .map(|member| self.strings.resolve(*member).to_string())
            .collect();
        members.sort();
        members
    }
}

/// ① Both anchors answer the element's union: the introduction's row and the
/// augment's shell row are two views of one member face.
#[test]
fn the_element_face_is_the_union_of_its_fragments() {
    let fixture = library(&[
        ("main.dart", "class C { int introMember = 1; }"),
        (
            "aug.dart",
            "part of 'main.dart';\naugment class C { int augmentMember = 2; }",
        ),
    ]);
    let expected = vec!["augmentMember".to_string(), "introMember".to_string()];
    assert_eq!(fixture.members(fixture.files[0], "C"), expected);
    assert_eq!(fixture.members(fixture.files[1], "C"), expected);
}

/// ⑤ The same fragments in a different unit order answer the same face: the
/// merge is keyed on the introduction's identity, never on which unit the
/// build happened to walk first.
#[test]
fn the_fragment_merge_is_order_independent() {
    let intro = ("main.dart", "class C { int a = 1; }");
    let first = (
        "one.dart",
        "part of 'main.dart';\naugment class C { int b = 2; }",
    );
    let last = (
        "two.dart",
        "part of 'main.dart';\naugment class C { int c = 3; }",
    );
    let forward = library(&[intro, first, last]);
    let reversed = library(&[intro, last, first]);
    let expected = vec!["a".to_string(), "b".to_string(), "c".to_string()];
    for fixture in [&forward, &reversed] {
        for &file in &fixture.files {
            assert_eq!(fixture.members(file, "C"), expected);
        }
    }
}

/// ② An introduction member read from inside an augment body resolves to the
/// element's face — the augment is a fragment, not a second element.
#[test]
fn introduction_member_reads_inside_an_augment_body() {
    let report = report_ops_augment(
        "class C { int introMember = 1; }\n",
        "augment class C { int readIt() => this.introMember; }\n",
    );
    assert!(
        !code_names(&report).contains(&"undefined_getter"),
        "{:#?}",
        rows(&report)
    );
}

/// ③ An augment-declared member read from the introducing unit's body
/// resolves the same way.
#[test]
fn augment_member_reads_from_the_introducing_body() {
    let report = report_ops_augment_intro_experiments(
        "class C { int readIt() => this.augmentMember; }\n",
        "augment class C { int augmentMember = 2; }\n",
        &[],
    );
    assert!(
        !code_names(&report).contains(&"undefined_getter"),
        "{:#?}",
        rows(&report)
    );
}

/// ④ A member declared in NEITHER fragment still reports — the union widens
/// the face to the element's real members, never to "anything resolves".
#[test]
fn a_member_declared_in_neither_fragment_still_reports() {
    let intro = "class C { int readIt() => this.absentMember; }\n";
    let augment = "augment class C { int augmentMember = 2; }\n";
    for report in [
        report_ops_augment_intro_experiments(intro, augment, &[]),
        report_ops_augment(
            "class C { int introMember = 1; }\n",
            "augment class C { int readIt() => this.absentMember; }\n",
        ),
    ] {
        let reported = rows(&report);
        assert!(
            reported
                .iter()
                .any(|row| row.contains("undefined_getter") && row.contains("'absentMember'")),
            "{reported:#?}"
        );
    }
}
