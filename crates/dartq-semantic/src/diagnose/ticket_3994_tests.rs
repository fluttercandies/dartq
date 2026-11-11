//! #3994 pins — the clause-argument scanner's keyword-headed function
//! tails: `scan_type_args`'s `dynamic`/`void` arms carry the same
//! `Function()` literal tail the identifier arm does, so a clause argument
//! spelled `dynamic Function()` or `void Function()` parses faithful and
//! stores a function-typed written argument — the substitution facts the
//! merged-column faces read.
//!
//! * `B<dynamic Function()>` / `B<void Function()>` type arguments now
//!   reach `clause_instantiation` as one `Ty::Function` whose return is
//!   the keyword type itself (upstream Dart SDK source,
//!   `pkg/_fe_analyzer_shared/lib/src/parser/type_info.dart`
//!   `computeType`: `void` answers `voidType` then reads `Function …` as
//!   a generalized function type, and `dynamic` takes the same tail
//!   through `isValidNonRecordTypeReference` — a keyword-headed function
//!   type is legal Dart). Before the tail the keyword arms committed the
//!   bare atom and the separator match refused `Function`, marking the
//!   whole header unfaithful — every substitution environment over the
//!   `implements B<dynamic Function()>` edge undecidable.
//!
//! The pins assert the scanner layer directly: `clause_instantiation` is
//! the face every downstream substitution reads, and a faithful header is
//! the only way it answers `Some`.

use super::*;
use crate::collector::collect;
use crate::library::ElementIndex;
use crate::model::{MicroKind, Ty};
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_resolution::{LibraryClassModifiers, LibraryGraphBuilder, Namespaces};
use dartq_scanner::ScanConfig;
use dartq_source::{FileTable, StringTable};
use std::path::Path;

/// The clause scanner over `source`, answered through a minimally wired
/// [`Engine`]: the same single-file `package:t` rig `report_ops` mounts,
/// stopped before the diagnose fold so `clause_instantiation` reads the
/// token walk's own verdict. `container`/`super_name` name the scanned
/// edge; `assert` runs inside the engine's lifetime because the written
/// arguments intern into its boundary pool.
fn with_clause_args(
    source: &str,
    container: &str,
    super_name: &str,
    assert: impl FnOnce(&Engine<'_>, Option<Vec<Ty>>),
) {
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
    assert!(
        scanned.diagnostics.is_empty(),
        "scanner diagnostics: {source:?}"
    );
    let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(
        LanguageVersion::new(3, 15),
    ));
    let elements = collect(source, &scanned.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
    let mut index = ElementIndex::default();
    index.insert(file, elements.clone());
    for (name, decl) in elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib, name, file);
    }
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let class_modifiers = LibraryClassModifiers::default();
    let engine = Engine::new(
        &graph,
        &namespaces,
        &index,
        &strings,
        source,
        &scanned.tokens,
        file,
        lib,
        features.primary_constructors_enabled,
        features.augmentations_enabled,
        features.variance_enabled,
        false,
        features.patterns_enabled,
        StrictModes::default(),
        &class_modifiers,
        None,
    );
    let stored = engine.clause_instantiation(strings.intern(container), strings.intern(super_name));
    // The stored face is `vec![whole_named_ty]` — the real argument list
    // is the stored `Ty::Named`'s own `args` (the same unwrap `face_args`
    // / `mixin_env` keeps).
    let args = stored.as_deref().and_then(|stored| match stored {
        [Ty::Named { args, .. }] => Some(engine.boundary.pool().get(*args).to_vec()),
        _ => None,
    });
    assert(&engine, args);
}

/// The scanner-level pin: `B<dynamic Function()>` parses a faithful
/// `implements` clause whose one written argument is the function type
/// `dynamic Function()` — the literal's return column is the `dynamic`
/// keyword itself. Before the keyword arms carried the tail, the
/// separator match refused `Function`, the header scanned unfaithful, and
/// `clause_instantiation` answered `None`.
#[test]
fn dynamic_function_clause_argument_parses_faithful() {
    with_clause_args(
        "class B<T> { T member() => throw 'unreachable'; }\n\
         class DND1 extends Object implements B<dynamic Function()> {}\n",
        "DND1",
        "B",
        |engine, args| {
            let args = args.expect("the `B<dynamic Function()>` header scans faithful");
            assert_eq!(args.len(), 1, "one written argument, got {args:?}");
            let Ty::Function { shape, nullable } = args[0] else {
                panic!("the written argument is `dynamic Function()`, got {args:?}");
            };
            assert!(!nullable, "the literal carries no `?`");
            let pool = engine.boundary.pool();
            let fn_shape = pool.function_shape(shape);
            assert_eq!(
                fn_shape.return_ty,
                Ty::Dynamic,
                "the literal's return column is the `dynamic` keyword itself"
            );
            assert!(
                fn_shape.positional.is_empty() && fn_shape.named.is_empty(),
                "the `()` literal carries no formals, got {:?}",
                fn_shape,
            );
        },
    );
}

/// The `void` keyword arm carries the same tail: `B<void Function()>`
/// parses faithful, the argument interning `void Function()`.
#[test]
fn void_function_clause_argument_parses_faithful() {
    with_clause_args(
        "class B<T> { T member() => throw 'unreachable'; }\n\
         class DND1 extends Object implements B<void Function()> {}\n",
        "DND1",
        "B",
        |engine, args| {
            let args = args.expect("the `B<void Function()>` header scans faithful");
            assert_eq!(args.len(), 1, "one written argument, got {args:?}");
            let Ty::Function { shape, .. } = args[0] else {
                panic!("the written argument is `void Function()`, got {args:?}");
            };
            let pool = engine.boundary.pool();
            let fn_shape = pool.function_shape(shape);
            assert_eq!(
                fn_shape.return_ty,
                Ty::Void,
                "the literal's return column is the `void` keyword itself"
            );
        },
    );
}

/// The plain keyword argument keeps its bare face: `B<dynamic>` still
/// scans `Ty::Dynamic`. A `Function` that is not the `()` literal tail
/// (`dynamic Function(int)`) stays unmodelable — the separator's `None`
/// posture, unchanged: the whole header keeps the unfaithful suppression
/// it always had.
#[test]
fn bare_keyword_stays_atom_and_spelled_formals_stay_unmodelable() {
    with_clause_args(
        "class B<T> { T member() => throw 'unreachable'; }\n\
         class DND1 extends Object implements B<dynamic> {}\n",
        "DND1",
        "B",
        |_engine, args| {
            assert_eq!(
                args.expect("the `B<dynamic>` header scans faithful"),
                vec![Ty::Dynamic],
                "bare `dynamic` stays the atom"
            );
        },
    );
    with_clause_args(
        "class B<T> { T member() => throw 'unreachable'; }\n\
         class DND1 extends Object implements B<dynamic Function(int)> {}\n",
        "DND1",
        "B",
        |_engine, args| {
            assert_eq!(
                args, None,
                "`dynamic Function(int)` keeps the unmodelable-shape posture"
            );
        },
    );
}
