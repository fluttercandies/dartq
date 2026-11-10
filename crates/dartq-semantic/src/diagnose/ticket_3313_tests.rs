//! #3313 pins — the shared member-resolution substrate behind the
//! `undefined_class` / `undefined_method` / `undefined_getter` cluster.
//!
//! # The cluster (corpus census)
//!
//! `cases`/`fail` are the tracked `status.tsv` columns (2026-11-01); the two
//! `rows` columns are the per-case diff rows of the last full sweep report
//! (2026-10-31), so they describe shapes rather than case denominators.
//!
//! | code | cases | fail | rows missing | rows extra |
//! |---|---|---|---|---|
//! | `undefined_class` | 1079 | 153 | 56 | 116 |
//! | `undefined_method` | 370 | 259 | 29 | 514 |
//! | `undefined_getter` | 421 | 224 | 147 | 310 |
//!
//! The EXTRA side dominates (940 rows against 232 missing), and one shape
//! owns it: a member access whose receiver the engine types as a FUNCTION
//! shape reaches the receiver-side member walk, which asks for the container
//! named `Function` — an unindexed `dart:core` unit — and answers
//! `Walk::Missing`, so the face reports
//! `The method 'x' isn't defined for the type 'Function'.` (337 rows across
//! 111 cases) where the oracle either stays silent (the receiver is
//! `dynamic`) or names the type it actually inferred. Probe:
//! `T castObject<T>(Object value) => value as T;` with
//! `(castObject(true)..whatever())` reports `'bool'` upstream and
//! `'Function'` here at the identical position — so the mismatch is a
//! receiver-TYPE defect, not a member-walk miss.
//!
//! # What these pins hold
//!
//! All three faces fold the same substrate: [`resolve_container`] (the
//! container lookup a type-use verdict and a receiver walk both read) and
//! [`member_lookup`] / [`member_lookup_defining`] (the superclass → mixin →
//! interface walk; upstream Dart SDK source, pkg/analyzer/lib/src/dart/
//! resolver/inheritance_manager3.dart — the application order at :571-579).
//! Both answers are `Option`-shaped on purpose: a `None` is *not* proof of
//! absence, and the over-report direction the cluster shows is exactly a
//! caller reading `None` as a report. The pins lock both halves — the found
//! faces (inherited, mixed-in, interfaced) and the silent-negative faces (a
//! broken supertype chain, a cyclic chain, a non-container name) — so a
//! change here cannot silently turn a suppression into a report.
//!
//! The receiver-type root above lives outside this substrate (the send's
//! callee-return face and the shape-receiver verdict); these pins are the
//! regression floor the fixing lane must keep green.

use crate::collector::collect;
use crate::library::{ElementIndex, member_lookup, member_lookup_defining, resolve_container};
use crate::model::{FileElements, MemberKind, MicroKind};
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_resolution::{Combinators, ImportEdge, LibraryGraphBuilder, Namespaces};
use dartq_scanner::{ScanConfig, TokenTable};
use dartq_source::{FileTable, StringTable};
use std::path::Path;

/// Scans + collects one unit, interning its names into `strings`.
fn unit(source: &str, strings: &StringTable, files: &mut FileTable, path: &str) -> FileElements {
    let file = files.intern(Path::new(path));
    let result = dartq_scanner::scan(
        source,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        file,
    );
    assert!(result.diagnostics.is_empty(), "{source:?}");
    let tokens: &'static TokenTable = Box::leak(Box::new(result.tokens));
    let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(
        LanguageVersion::new(3, 15),
    ));
    collect(source, tokens, features, strings)
}

/// The walk's three found faces in one chain: `class C extends B with M
/// implements I` answers `m` through the superclass edge, `x` through the
/// mixin application, and `i` through the interface face, which the
/// `extends` chain reaches only after it ends (an implementor inherits its
/// interfaces' members — the oracle's interface build).
#[test]
fn member_walk_answers_superclass_mixin_and_interface_faces() {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let base = unit(
        "class B { void m() {} } \
         mixin M { void x() {} } \
         abstract class I { void i(); }",
        &strings,
        &mut files,
        "base.dart",
    );
    let derived = unit(
        "class C extends B with M implements I {}",
        &strings,
        &mut files,
        "use.dart",
    );
    let base_file = files.try_file_id(Path::new("base.dart")).unwrap();
    let use_file = files.try_file_id(Path::new("use.dart")).unwrap();

    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/lib.dart");
    for (file, elements) in [(base_file, &base), (use_file, &derived)] {
        for (name, decl) in elements.declarations() {
            if decl.kind != MicroKind::TypeParam {
                builder.declare(lib, name, file);
            }
        }
    }
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let mut index = ElementIndex::default();
    index.insert(base_file, base.clone());
    index.insert(use_file, derived);

    let container = strings.intern("C");
    assert!(
        member_lookup(
            &graph,
            &namespaces,
            &index,
            lib,
            use_file,
            container,
            strings.intern("m"),
        )
        .is_some(),
        "inherited through the superclass edge"
    );
    assert!(
        member_lookup(
            &graph,
            &namespaces,
            &index,
            lib,
            use_file,
            container,
            strings.intern("x"),
        )
        .is_some(),
        "mixed in through the `with` clause"
    );
    assert!(
        member_lookup(
            &graph,
            &namespaces,
            &index,
            lib,
            use_file,
            container,
            strings.intern("i"),
        )
        .is_some(),
        "inherited through the `implements` face"
    );
    // The report signal: the name is nowhere on the chain.
    assert!(
        member_lookup(
            &graph,
            &namespaces,
            &index,
            lib,
            use_file,
            container,
            strings.intern("absent"),
        )
        .is_none()
    );
}

/// The defining container is a walk fact, not decoration: `m` is declared on
/// `B` while `own` is the anchor's own, so the declaration-side queries can
/// tell an inherited hit from a self-declaration.
#[test]
fn member_walk_reports_the_defining_container() {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let elements = unit(
        "class B { void m() {} } class C extends B { void own() {} }",
        &strings,
        &mut files,
        "main.dart",
    );
    let file = files.try_file_id(Path::new("main.dart")).unwrap();

    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/lib.dart");
    for (name, decl) in elements.declarations() {
        if decl.kind != MicroKind::TypeParam {
            builder.declare(lib, name, file);
        }
    }
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let mut index = ElementIndex::default();
    index.insert(file, elements);

    let container = strings.intern("C");
    let inherited = member_lookup_defining(
        &graph,
        &namespaces,
        &index,
        lib,
        file,
        container,
        strings.intern("m"),
        false,
    )
    .expect("inherited member");
    assert_eq!(
        inherited.0,
        strings.intern("B"),
        "the hit names its declarer"
    );
    assert_eq!(inherited.1.kind, MemberKind::Method);

    let declared = member_lookup_defining(
        &graph,
        &namespaces,
        &index,
        lib,
        file,
        container,
        strings.intern("own"),
        false,
    )
    .expect("self-declared member");
    assert_eq!(
        declared.0, container,
        "a self-declaration names the anchor itself"
    );
}

/// A supertype name that resolves to nothing indexed leaves the chain
/// unknowable — the answer stays `None` even for a name the missing face
/// could carry. This is the suppression the family depends on: an
/// unanalysed supertype must never be read as "the member is absent".
#[test]
fn member_walk_is_silent_on_a_broken_supertype_face() {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let elements = unit(
        "class C extends Missing {} \
         class B { void m() {} } \
         class D extends B with Missing {}",
        &strings,
        &mut files,
        "main.dart",
    );
    let file = files.try_file_id(Path::new("main.dart")).unwrap();

    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/lib.dart");
    for (name, decl) in elements.declarations() {
        if decl.kind != MicroKind::TypeParam {
            builder.declare(lib, name, file);
        }
    }
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let mut index = ElementIndex::default();
    index.insert(file, elements);

    // The unresolved superclass could carry `m` — silence, never a report.
    assert!(
        member_lookup(
            &graph,
            &namespaces,
            &index,
            lib,
            file,
            strings.intern("C"),
            strings.intern("m"),
        )
        .is_none()
    );
    // An unresolved mixin leaves nothing to match on either — the answer
    // stays `None` even though the superclass written before it resolved.
    assert!(
        member_lookup(
            &graph,
            &namespaces,
            &index,
            lib,
            file,
            strings.intern("D"),
            strings.intern("x"),
        )
        .is_none()
    );
    // The resolvable part of D's chain still answers.
    assert!(
        member_lookup(
            &graph,
            &namespaces,
            &index,
            lib,
            file,
            strings.intern("D"),
            strings.intern("m"),
        )
        .is_some()
    );
}

/// A cyclic supertype chain is malformed input, not a hang: the walk answers
/// `None` on the second visit of a name it already climbed.
#[test]
fn member_walk_cycle_guard_terminates() {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let elements = unit(
        "class A extends B {} class B extends A {}",
        &strings,
        &mut files,
        "main.dart",
    );
    let file = files.try_file_id(Path::new("main.dart")).unwrap();

    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/lib.dart");
    for (name, decl) in elements.declarations() {
        if decl.kind != MicroKind::TypeParam {
            builder.declare(lib, name, file);
        }
    }
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let mut index = ElementIndex::default();
    index.insert(file, elements);

    assert!(
        member_lookup(
            &graph,
            &namespaces,
            &index,
            lib,
            file,
            strings.intern("A"),
            strings.intern("m"),
        )
        .is_none()
    );
}

/// The container lookup a type-use verdict reads: the diagnosing file's own
/// declarations first, then the library namespace (the import face), and
/// `None` for a name that is not a container at all — the "not a type"
/// signal, never a report on its own.
#[test]
fn container_lookup_is_file_local_then_namespace() {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let elements_a = unit(
        "class Base { void m() {} } void fn() {}",
        &strings,
        &mut files,
        "a.dart",
    );
    let elements_b = unit(
        "import 'a.dart'; class Derived extends Base {}",
        &strings,
        &mut files,
        "b.dart",
    );
    let file_a = files.try_file_id(Path::new("a.dart")).unwrap();
    let file_b = files.try_file_id(Path::new("b.dart")).unwrap();

    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib_a = builder.intern("package:t/a.dart");
    let lib_b = builder.intern("package:t/b.dart");
    let mut index = ElementIndex::default();
    for (lib, file, elements) in [(lib_a, file_a, &elements_a), (lib_b, file_b, &elements_b)] {
        index.insert(file, elements.clone());
        for (name, decl) in elements.declarations() {
            if decl.kind != MicroKind::TypeParam {
                builder.declare(lib, name, file);
            }
        }
    }
    builder.import(
        lib_b,
        ImportEdge {
            target: Some(lib_a),
            prefix: None,
            deferred: false,
            non_library: false,
            combinators: Combinators::unfiltered(),
        },
    );
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);

    let (_, own) = resolve_container(
        &graph,
        &namespaces,
        &index,
        lib_b,
        file_b,
        strings.intern("Derived"),
    )
    .expect("declared in the diagnosing file");
    assert_eq!(own, file_b, "the local file owns its own container");

    let (_, imported) = resolve_container(
        &graph,
        &namespaces,
        &index,
        lib_b,
        file_b,
        strings.intern("Base"),
    )
    .expect("imported through the namespace");
    assert_eq!(imported, file_a, "the namespace answers the owner file");

    assert!(
        resolve_container(
            &graph,
            &namespaces,
            &index,
            lib_b,
            file_b,
            strings.intern("Nope"),
        )
        .is_none(),
        "an unknown name answers nothing"
    );
    assert!(
        resolve_container(
            &graph,
            &namespaces,
            &index,
            lib_b,
            file_b,
            strings.intern("fn"),
        )
        .is_none(),
        "a top-level function is not a container"
    );
}
