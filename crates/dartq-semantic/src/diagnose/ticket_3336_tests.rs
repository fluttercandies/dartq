//! #3336 pins — a top-level function's and a type alias's own
//! `<T extends E>` group reaches the resolved bound ledger.
//!
//! The defect (#3328 residual): the collector's `attach_type_param_count`
//! consumed the depth-1 group for a pending top-level executable and a
//! pending alias, but the bound half (`type_var_bounds`) was taken and
//! dropped — only the spellings reached the element model. Container
//! headers and members always carried both columns
//! (`Container::type_param_bounds`, `MemberEntry::type_param_bounds`), so
//! only the top-level kinds lost the bound.
//!
//! The consumer is `resolved_ops`' file-scoped bound ledger, which
//! `ResolvedTypeOps::is_always_exhaustive` reads through its type-parameter
//! arm (`type_system.dart:873-879`): a `T extends St` over the enum `St`
//! splits completely, so a switch over `T` closes. Without the bound the
//! ledger answered the unbounded face and the switch stayed open. The
//! diagnose pipeline's own exhaustiveness face is a different transport
//! (the recorder's `boundaries` table), which is why this defect never
//! surfaced in a `FileReport` pin — the pins below therefore assert on the
//! adapter the engine's fold mounts, not on emitted rows.
//!
//! The persisted declaration payload carries the spellings only, so a model
//! restored from a payload keeps the damped face: that is the
//! documented residual, not a pin here.
//!
//! Registered through `diagnose/tests.rs`'s `#[path]` block: `diagnose.rs`
//! is a live-lane file out of this ticket's write scope.

use std::path::Path;

use dartq_flow::ty::{Base, EMPTY_ARGS, Ty as FlowTy, TypeOps};
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_resolution::{LibraryGraphBuilder, Namespaces};
use dartq_scanner::ScanConfig;
use dartq_source::{FileId, FileTable, StringTable};

use crate::collector::collect;
use crate::library::ElementIndex;
use crate::model::MicroKind;
use crate::resolved_ops::{LibraryTypes, ResolvedTypeOps};

/// The recorder's name table the adapter's `Base::Name` ids index into: the
/// fixture's enum and its type-parameter spelling.
const UNIVERSE: &[&str] = &["St", "T"];

/// `St`'s row in [`UNIVERSE`].
const ST: u32 = 0;

/// `T`'s row in [`UNIVERSE`].
const PARAM: u32 = 1;

/// One unit's resolved snapshot plus its adapter, leaked to the `'static`
/// the adapter's own borrows want (the pin owns them for its whole body).
struct Fixture {
    /// The snapshot the bound ledger (`param_bound`) belongs to.
    types: &'static LibraryTypes,
    /// The adapter the engine's fold mounts — the `isAlwaysExhaustive` face
    /// over the same ledger.
    ops: ResolvedTypeOps<'static>,
    file: FileId,
    strings: &'static StringTable,
}

impl Fixture {
    /// The bound head the ledger answers for `spelling` in this unit.
    fn bound_head(&self, spelling: &str) -> Option<&str> {
        self.types
            .param_bound(self.file, self.strings.intern(spelling))
            .map(|head| self.strings.resolve(head))
    }

    /// `isAlwaysExhaustive` over [`UNIVERSE`]'s `index` row.
    fn exhaustive(&self, index: u32) -> bool {
        let names: Vec<Box<str>> = UNIVERSE.iter().map(|name| (*name).into()).collect();
        let arena = vec![Vec::new()];
        self.ops.is_always_exhaustive(
            FlowTy {
                base: Base::Name(index),
                nullable: false,
            },
            EMPTY_ARGS,
            &arena,
            &names,
        )
    }
}

/// Scans, collects and resolves one unit as the sole file of its library.
fn fixture(source: &str) -> Fixture {
    let strings: &'static StringTable = Box::leak(Box::new(StringTable::new()));
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
    assert!(scanned.diagnostics.is_empty(), "{source:?}");
    let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(
        LanguageVersion::new(3, 15),
    ));
    let elements = collect(source, &scanned.tokens, features, strings);
    let mut builder = LibraryGraphBuilder::new(strings);
    let lib = builder.intern("package:t/main.dart");
    builder.unit(lib, file);
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
    let types: &'static LibraryTypes = Box::leak(Box::new(LibraryTypes::build(
        &graph,
        &namespaces,
        &index,
        strings,
        lib,
    )));
    Fixture {
        types,
        ops: ResolvedTypeOps::new(types, strings, file),
        file,
        strings,
    }
}

/// The defect itself: a generic top-level function's group bound now rides
/// the element model (`TopDecl::type_param_bounds`) and lands in the
/// file-scoped ledger. Before the fix the spelling registered with no
/// bound at all.
#[test]
fn top_level_fn_group_bound_reaches_the_ledger() {
    let f = fixture("enum St { a, b }\nvoid f<T extends St>(T v) {}\n");
    assert_eq!(f.bound_head("T"), Some("St"));
}

/// The behavioural half: the ledger's bound closes the switch the way the
/// official `TypeParameterTypeImpl` arm does (`T extends St` over the enum
/// `St` is always exhaustive). This is the `is_always_exhaustive` answer an
/// unbound ledger cannot give.
#[test]
fn bounded_top_level_fn_param_closes_the_switch() {
    let f = fixture("enum St { a, b }\nvoid f<T extends St>(T v) {}\n");
    assert!(
        f.exhaustive(PARAM),
        "a `St`-bounded top-level parameter must split completely"
    );
}

/// A type alias's own group bounds the same way (the pending-typedef arm
/// takes its bound list at the same first depth-1 close).
#[test]
fn typedef_group_bound_reaches_the_ledger() {
    let f = fixture("enum St { a, b }\ntypedef G<T extends St> = T Function();\n");
    assert_eq!(f.bound_head("T"), Some("St"));
}

/// The conservative face survives: an unbounded `T` keeps the implicit
/// `Object?` reading — no ledger entry, and the sweep never closes.
#[test]
fn unbounded_top_level_fn_group_stays_damped() {
    let f = fixture("enum St { a, b }\nvoid g<T>(T v) {}\n");
    assert_eq!(f.bound_head("T"), None);
    assert!(
        !f.exhaustive(PARAM),
        "an unbounded parameter must never close a switch"
    );
}

/// Two declarations of one spelling that disagree poison the ledger rather
/// than borrow each other's bound (the merge rule the container faces keep).
#[test]
fn disagreeing_group_bounds_poison_the_spelling() {
    let f = fixture(
        "enum St { a, b }\nvoid f<T extends St>(T v) {}\nvoid g<T extends Object>(T v) {}\n",
    );
    assert_eq!(f.bound_head("T"), None);
    assert!(!f.exhaustive(PARAM));
}
