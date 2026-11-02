//! #3292 pins — the shared root behind five `unnecessary_statements`
//! residuals plus the `non_bool_condition` for-header face.
//!
//! # The defect
//!
//! A constructor-invocation receiver types as the `Type` metaclass (the
//! TypeLiteral face of a container spelling in value position), and the
//! `dart:core` containers resolve through the library namespace onto
//! unindexed (external) units, so [`member_lookup`] chain-breaks there.
//! Together those two faces made the getter-vs-tear-off split unreachable
//! for every SDK receiver: `Analysis::member_of` answered `None` for
//! `List.empty().first` (the oracle's silent getter face — the lint then
//! reported) and for `List.empty().where` (the oracle's method tear-off —
//! the lint then stayed silent). `List.empty().g` over an in-file class
//! failed the same way through the `Type` metaclass short-circuit.
//!
//! [`member_lookup`]: crate::library::member_lookup
//!
//! # The fix
//!
//! [`Analysis::member_of`] no longer treats a `Type`-typed receiver as the
//! lookup container (the site's own reference row names it), and the
//! member walk answers the unindexed core containers through the
//! name-level kind face ([`crate::member_face::core_member_entry`]): the
//! entry claims the member's KIND and no declared type — the under-report
//! direction `member_type` documents.
//!
//! Oracle provenance (`dart analyze` 3.13.1): `List.empty().first;` is
//! silent, `List.empty()..where` reports `unnecessary_statements` at the
//! section, `Type t = List;` types the read `Type`, and a C-style `for`
//! condition reports `non_bool_condition` at the condition expression
//! alone (its updaters are never conditions).

use crate::analysis::{Analysis, analyze_unit};
use crate::diagnose::StrictModes;
use crate::library::ElementIndex;
use crate::model::{MemberKind, MicroKind, Ty};
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_resolution::{
    Combinators, ImportEdge, LibraryClassModifiers, LibraryGraph, LibraryGraphBuilder, Namespaces,
};
use dartq_scanner::{ScanConfig, TokenTable};
use dartq_source::{FileId, FileTable, SourceRange, StringTable};
use std::path::Path;

use super::test_rig::{code_names, report_ops};

/// The names-only `dart:core` face the fixtures resolve through: the
/// spellings land on a file the element index never carries — exactly the
/// engine graph's SDK posture (`dart:core`'s declarations are external, so
/// the member walk chain-breaks and only the name-level kind face answers).
const CORE_FACE: [&str; 6] = ["List", "Map", "Set", "Iterable", "Type", "String"];

/// Builds the [`Analysis`] of `source` as the sole indexed unit of
/// `package:t`, with [`CORE_FACE`] mounted as an imported, unindexed
/// library. The context tables leak (`'static`) — the tests are one-shot
/// processes.
fn analyze_core(source: &str) -> Analysis<'static> {
    let strings: &'static StringTable = Box::leak(Box::new(StringTable::new()));
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(
        LanguageVersion::new(3, 15),
    ));
    let scan_config = ScanConfig {
        enable_triple_shift: true,
        enable_augmentations: true,
    };
    let scanned = dartq_scanner::scan(source, scan_config, file);
    assert!(
        scanned.diagnostics.is_empty(),
        "scanner diagnostics: {source:?}"
    );
    let tokens: &'static TokenTable = Box::leak(Box::new(scanned.tokens));
    let elements = crate::collector::collect(source, tokens, features, strings);
    let mut builder = LibraryGraphBuilder::new(strings);
    let lib = builder.intern("package:t/main.dart");
    builder.unit(lib, file);
    for (name, decl) in elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib, name, file);
    }
    let mut index = ElementIndex::default();
    index.insert(file, elements);
    // The core face: one library whose declarations resolve but whose file
    // the index never carries (the external-unit posture).
    let core_file: FileId = files.intern(Path::new("dart_core.dart"));
    let core_lib = builder.intern("dart:core");
    builder.unit(core_lib, core_file);
    for name in CORE_FACE {
        builder.declare(core_lib, strings.intern(name), core_file);
    }
    builder.import(
        lib,
        ImportEdge {
            target: Some(core_lib),
            prefix: None,
            deferred: false,
            non_library: false,
            combinators: Combinators::unfiltered(),
        },
    );
    let graph: &'static LibraryGraph = Box::leak(Box::new(builder.finish()));
    let namespaces: &'static Namespaces = Box::leak(Box::new(Namespaces::build(graph)));
    let index: &'static ElementIndex = Box::leak(Box::new(index));
    analyze_unit(
        source,
        tokens,
        features,
        strings,
        file,
        lib,
        graph,
        namespaces,
        index,
        StrictModes::default(),
        &LibraryClassModifiers::default(),
    )
}

/// The [`SourceRange`] of `needle`'s first occurrence (ASCII fixtures: byte
/// offsets are UTF-16 offsets).
#[expect(clippy::cast_possible_truncation)] // span arithmetic narrows under the scanner u32 offset cap
fn span_of(source: &str, needle: &str) -> SourceRange {
    let offset: u32 = source
        .find(needle)
        .unwrap_or_else(|| panic!("{needle:?} not in {source:?}"))
        .try_into()
        .expect("offset fits u32");
    SourceRange {
        offset,
        length: needle.len() as u32,
    }
}

/// The kind `Analysis::member_of` answers at the receiver `receiver`'s own
/// span for `member`.
fn member_kind(
    analysis: &Analysis<'_>,
    source: &str,
    receiver: &str,
    member: &str,
) -> Option<MemberKind> {
    analysis
        .member_of(span_of(source, receiver), analysis.strings().intern(member))
        .map(|binding| binding.entry.kind)
}

/// `List.empty().first` reads a GETTER (`sdk/lib/core/iterable.dart`'s
/// `E get first`): the constructor-invocation receiver must reach the
/// container's member face, never the `Type` metaclass's. The
/// `unnecessary_statements` getter face stays silent on this shape
/// (`test_instanceGetter`).
#[test]
fn ticket_3292_ctor_receiver_first_is_a_getter() {
    let source = "void f() { List.empty().first; }";
    let analysis = analyze_core(source);
    assert_eq!(
        member_kind(&analysis, source, "List", "first"),
        Some(MemberKind::Getter),
        "List.empty().first must resolve the getter face"
    );
}

/// `List.empty().where` reads a METHOD — the cascaded-section tear-off the
/// `unnecessary_statements` cascade face reports
/// (`test_methodTearoff_cascaded` and its `followOn`/local/top-level
/// `return` siblings).
#[test]
fn ticket_3292_ctor_receiver_where_is_a_method() {
    let source = "void f() { List.empty().where; }";
    let analysis = analyze_core(source);
    assert_eq!(
        member_kind(&analysis, source, "List", "where"),
        Some(MemberKind::Method),
        "List.empty().where must resolve the tear-off face"
    );
}

/// The SDK kind face covers the plain receiver shapes too: a `List<int>`
/// formal's `length` is a getter, its `add` a method.
#[test]
fn ticket_3292_core_member_kinds_split_getters_from_methods() {
    let source = "void f(List<int> xs) { xs.length; xs.add(1); }";
    let analysis = analyze_core(source);
    assert_eq!(
        member_kind(&analysis, source, "xs.length", "length"),
        Some(MemberKind::Getter),
        "List.length"
    );
    assert_eq!(
        member_kind(&analysis, source, "xs.add", "add"),
        Some(MemberKind::Method),
        "List.add"
    );
}

/// The same-file class face keeps its split through the creation receiver
/// (`C().g` is a getter): the `Type` metaclass never shadows the container
/// the reference row names.
#[test]
fn ticket_3292_creation_receiver_getter_over_a_class() {
    let source = "class C { int get g => 0; int m() => 1; }\n\
                  void f() { C().g; C().m; }";
    let analysis = analyze_core(source);
    assert_eq!(
        member_kind(&analysis, source, "C().g", "g"),
        Some(MemberKind::Getter)
    );
    assert_eq!(
        member_kind(&analysis, source, "C().m", "m"),
        Some(MemberKind::Method)
    );
}

/// The synthesized SDK entry claims the KIND alone: `member_type` keeps its
/// under-report `None` (every declared type of the entry is `Unknown`), so
/// no consumer reads a fabricated return type.
#[test]
fn ticket_3292_core_member_type_underreports() {
    let source = "void f() { List.empty().first; }";
    let analysis = analyze_core(source);
    assert_eq!(
        analysis.member_type(span_of(source, "List"), analysis.strings().intern("first")),
        None,
        "the kind face claims no declared type"
    );
}

/// The TypeLiteral face stands: a `Type`-declared read types `Type`, and a
/// member asked of it under-reports (the metaclass carries no core
/// container's members) — only the container SPELLING resolves its own
/// member face.
#[test]
fn ticket_3292_type_typed_read_stays_type() {
    let source = "void f() { Type t = List; t.toString; }";
    let analysis = analyze_core(source);
    let ty = analysis.type_of(span_of(source, "t.toString"));
    assert_eq!(
        ty,
        Some(Ty::named(analysis.strings().intern("Type"))),
        "a Type-declared read types Type"
    );
    assert_eq!(
        analysis.member_of(
            span_of(source, "t.toString"),
            analysis.strings().intern("toString")
        )
        .map(|binding| binding.entry.kind),
        None,
        "a Type-typed receiver answers no container member"
    );
}

/// The C-style `for` condition reports at the CONDITION expression, never
/// the updater region: `for (; C; D)` carries one `non_bool_condition`,
/// anchored on `C`.
#[test]
fn ticket_3292_non_bool_condition_anchors_the_condition() {
    let fixture = "class C {}\nclass D {}\nvoid f() { for (; C; D) {} }\n";
    let report = report_ops(fixture);
    let rows: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "non_bool_condition")
        .collect();
    assert_eq!(rows.len(), 1, "{:?}", code_names(&report));
    // The rig prefixes `CORE_PRELUDE`; both halves are ASCII, so the
    // fixture's base offset is the prelude's length.
    let base = u32::try_from(super::test_rig::CORE_PRELUDE.len()).expect("prelude fits u32");
    let at = u32::try_from(fixture.find("C; D").expect("condition present"))
        .expect("fixture offset fits u32");
    assert_eq!(rows[0].range.offset, base + at, "{:?}", rows[0]);
    assert_eq!(rows[0].range.length, 1, "{:?}", rows[0]);
}

/// An empty condition makes the updaters the only rows between the
/// separators — they are not conditions, so `for (;; C)` stays silent
/// (the parser hands the listener the token following the condition's own
/// `;`, which for the empty condition is the first updater).
#[test]
fn ticket_3292_non_bool_condition_skips_the_updaters() {
    let fixture = "class C {}\nvoid f() { for (;; C) {} }\n";
    let report = report_ops(fixture);
    assert!(
        !code_names(&report).contains(&"non_bool_condition"),
        "{:?}",
        code_names(&report)
    );
}
