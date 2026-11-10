//! #3398 pins — `Receiver::Created` carries the creation's resolved
//! instance type so `member_formals_full` substitutes the container
//! parameters the member's formals spell, the same face a
//! declaration-typed receiver already instantiates.
//!
//! # The defect
//!
//! `member_formals_full`'s `Receiver::Created` arm passed `recv_ty =
//! None`, so `Future<int>(1).then(cb)`'s formal table kept the bare `T`
//! (the greatest-closure `Object?` face downstream) instead of
//! substituting `T := int` from the creation's resolved type.
//! `Receiver::Created` gains `ty: Option<Ty>`; `Some` routes through the
//! same `member_recv_env` walk `Receiver::Typed` takes, `None` keeps the
//! pre-#3398 under-report (the construction-site population is #3394's).
//!
//! # The pins
//!
//! The arm is exercised directly: a hand-built `Receiver::Created`
//! against a single-file collected fixture, since the construction-site
//! population lands under a different lane.

use super::Receiver;
use crate::collector::collect;
use crate::diagnose::{Engine, StrictModes};
use crate::library::ElementIndex;
use crate::model::{ContainerKind, MicroKind, Ty};
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_resolution::{LibraryClassModifiers, LibraryGraphBuilder, Namespaces};
use dartq_scanner::ScanConfig;
use dartq_source::{FileTable, StringTable};
use std::path::Path;

/// The first positional formal's rendered type of `member` resolved
/// through `recv` — `"T"` for the unsubstituted declared table, `"int"`
/// once the receiver's instantiation substitutes it.
fn member_formal(
    source: &str,
    container: &str,
    member: &str,
    recv_ty_args: Option<&[&str]>,
) -> String {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let scanned = dartq_scanner::scan(
        source,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        file,
    );
    assert!(scanned.diagnostics.is_empty());
    let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(
        LanguageVersion::new(3, 15),
    ));
    let elements = collect(source, &scanned.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
    builder.unit(lib, file);
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
    let tokens: &'static dartq_scanner::TokenTable = Box::leak(Box::new(scanned.tokens));
    let posture = LibraryClassModifiers::default();
    let engine = Engine::new(
        &graph,
        &namespaces,
        &index,
        &strings,
        source,
        tokens,
        file,
        lib,
        false,
        false,
        false,
        false,
        true,
        StrictModes::default(),
        &posture,
        None,
    );
    // The receiver's instantiation interns in the engine's OWN boundary
    // pool — the same pool `member_recv_env` reads the written arguments
    // and `substitute_formals` interns the substituted table through.
    let ty = recv_ty_args.map(|args| {
        let args: Vec<Ty> = args
            .iter()
            .map(|arg| Ty::named(strings.intern(arg)))
            .collect();
        engine
            .boundary
            .args
            .borrow_mut()
            .named(strings.intern(container), false, &args)
    });
    let recv = Receiver::Created {
        name: strings.intern(container),
        kind: ContainerKind::Class,
        ty,
    };
    let formals = engine
        .member_formals_full(&recv, strings.intern(member))
        .expect("the member resolves through the created receiver");
    formals.positional[0]
        .ty
        .render(&strings, &engine.boundary.args.borrow())
}

/// The folded creation carries `Holder<int>`: `set`'s declared `T value`
/// formal instantiates to `int` — the oracle's member face for
/// `Holder<int>(x).set(...)`.
#[test]
fn t3398_created_ty_substitutes_the_member_formal() {
    let rendered = member_formal(
        "class Holder<T> { void set(T value) {} }",
        "Holder",
        "set",
        Some(&["int"]),
    );
    assert_eq!(rendered, "int");
}

/// A two-parameter container substitutes positionally: `first`'s `A`
/// formal takes the creation's first written argument.
#[test]
fn t3398_created_ty_zips_multi_param_positions() {
    let rendered = member_formal(
        "class Pair<A, B> { void first(A a, B b) {} }",
        "Pair",
        "first",
        Some(&["int", "String"]),
    );
    assert_eq!(rendered, "int");
}

/// Regression pin: `ty: None` keeps the pre-#3398 under-report — the
/// declared table comes back byte-identical with `T` unsubstituted until
/// the construction sites populate (`ty: Some(..)`).
#[test]
fn t3398_created_without_ty_keeps_the_declared_table() {
    let rendered = member_formal(
        "class Holder<T> { void set(T value) {} }",
        "Holder",
        "set",
        None,
    );
    assert_eq!(rendered, "T");
}
