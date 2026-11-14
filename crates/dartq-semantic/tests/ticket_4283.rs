//! Pin: the shared closure edge table (#4283) is observably equivalent to
//! the inline supertype-closure scan.
//!
//! `LibraryTypes::build_with_edges` replays the per-analyze
//! `ClosureEdges` table instead of re-collecting clause heads and
//! re-running declaring-scope lookups per library. The replay keeps the
//! per-library membership filters (first-writer-wins) and the visit
//! order, so the built snapshot — and every diagnostic the fold derives
//! from it — must be identical to `build_with_clauses`'s inline scan.
//!
//! The fixture makes the closure load-bearing: `use` lives in a library
//! that imports only `mid.dart`, so `Base` enters its registry solely
//! through the closure (mid's clause head resolving in mid's own scope to
//! base.dart). The `l is Base` test then reads through the
//! `Leaf -> Mid -> Base` chain and deads the else arm — a row that
//! disappears if either closure path under-registers.

use std::path::Path;

use dartq_language::{Feature, FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_resolution::{
    ClassModifiers, Combinators, ImportEdge, LibraryClassModifiers, LibraryGraphBuilder, Namespaces,
};
use dartq_scanner::{ScanConfig, scan};
use dartq_semantic::capture_clause_trees;
use dartq_semantic::collector::collect;
use dartq_semantic::diagnose::{StrictModes, fold_unit_with_ops};
use dartq_semantic::library::ElementIndex;
use dartq_semantic::model::MicroKind;
use dartq_semantic::resolved_ops::{
    ClauseTrees, ClosureEdges, LibraryTypes, ResolvedTypeOps,
};
use dartq_source::{FileId, FileTable, StringTable};

/// The synthetic dart:core names the fragments consume.
const CORE_FACE: &[&str] = &["Object", "bool", "int", "String"];

const BASE_UNIT: &str = "class Base {}\n";

const MID_UNIT: &str = "import 'package:t/base.dart';\n\nclass Mid extends Base {}\n";

const USE_UNIT: &str = "\
import 'package:t/mid.dart';

class Leaf extends Mid {}

void use(Leaf l) {
  if (l is Base) {
    print(1);
  } else {
    print(2);
  }
}
";

struct Workspace {
    strings: StringTable,
    files: FileTable,
    use_file: FileId,
    graph: dartq_resolution::LibraryGraph,
    namespaces: Namespaces,
    index: ElementIndex,
    clauses: ClauseTrees,
    core: dartq_resolution::LibraryId,
    use_lib: dartq_resolution::LibraryId,
}

fn workspace() -> Workspace {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let base_file = files.intern(Path::new("base.dart"));
    let mid_file = files.intern(Path::new("mid.dart"));
    let use_file = files.intern(Path::new("use.dart"));
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    let features = ParserFeatures::from_feature_set(set);
    let mut index = ElementIndex::default();
    let mut clauses = ClauseTrees::empty();
    let mut builder = LibraryGraphBuilder::new(&strings);
    let core = builder.intern("dart:core");
    let core_file = files.intern(Path::new("<dart:core>"));
    for name in CORE_FACE {
        builder.declare(core, strings.intern(name), core_file);
    }
    let mut libs = Vec::new();
    for (uri, file, source) in [
        ("package:t/base.dart", base_file, BASE_UNIT),
        ("package:t/mid.dart", mid_file, MID_UNIT),
        ("package:t/use.dart", use_file, USE_UNIT),
    ] {
        let lib = builder.intern(uri);
        let scanned = scan(
            source,
            ScanConfig {
                enable_triple_shift: true,
                enable_augmentations: set.is_enabled(Feature::Augmentations),
            },
            file,
        );
        assert!(
            scanned.diagnostics.is_empty(),
            "scanner diagnostics: {scanned:?}"
        );
        let elements = collect(source, &scanned.tokens, features, &strings);
        index.insert(file, elements.clone());
        builder.unit(lib, file);
        for (name, decl) in elements.declarations() {
            if decl.kind == MicroKind::TypeParam {
                continue;
            }
            if matches!(
                decl.kind,
                MicroKind::Class | MicroKind::Enum | MicroKind::Mixin
            ) {
                builder.declare_class(lib, name, file, ClassModifiers::default());
            } else {
                builder.declare(lib, name, file);
            }
        }
        capture_clause_trees(file, source, &scanned.tokens, features, &strings, &mut clauses);
        builder.import(
            lib,
            ImportEdge {
                target: Some(core),
                prefix: None,
                deferred: false,
                combinators: Combinators::unfiltered(),
                non_library: false,
            },
        );
        libs.push(lib);
    }
    let [base_lib, mid_lib, use_lib] = [libs[0], libs[1], libs[2]];
    builder.import(
        mid_lib,
        ImportEdge {
            target: Some(base_lib),
            prefix: None,
            deferred: false,
            combinators: Combinators::unfiltered(),
            non_library: false,
        },
    );
    builder.import(
        use_lib,
        ImportEdge {
            target: Some(mid_lib),
            prefix: None,
            deferred: false,
            combinators: Combinators::unfiltered(),
            non_library: false,
        },
    );
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    Workspace {
        strings,
        files,
        use_file,
        graph,
        namespaces,
        index,
        clauses,
        core,
        use_lib,
    }
}

/// Folds the `use` unit under the given snapshot, the way the engine
/// mounts one per library.
fn fold_report(ws: &Workspace, types: &LibraryTypes) -> dartq_semantic::FileReport {
    let ops = ResolvedTypeOps::new(types, &ws.strings, ws.use_file);
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    let features = ParserFeatures::from_feature_set(set);
    let scanned = scan(
        USE_UNIT,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: set.is_enabled(Feature::Augmentations),
        },
        ws.use_file,
    );
    fold_unit_with_ops(
        USE_UNIT,
        &scanned.tokens,
        features,
        &ws.strings,
        ws.use_file,
        ws.use_lib,
        &ws.graph,
        &ws.namespaces,
        &ws.index,
        StrictModes::default(),
        &LibraryClassModifiers::default(),
        false,
        Some(&ops),
    )
    .report
}

#[test]
fn closure_edge_table_matches_inline_scan() {
    let ws = workspace();
    let edges = ClosureEdges::build(&ws.graph, &ws.namespaces, &ws.index, &ws.clauses);
    let with_table = LibraryTypes::build_with_edges(
        &ws.graph,
        &ws.namespaces,
        &ws.index,
        &ws.strings,
        ws.use_lib,
        &ws.clauses,
        &edges,
    );
    let inline = LibraryTypes::build_with_clauses(
        &ws.graph,
        &ws.namespaces,
        &ws.index,
        &ws.strings,
        ws.use_lib,
        &ws.clauses,
    );
    let table_rows = fold_report(&ws, &with_table);
    let inline_rows = fold_report(&ws, &inline);
    let summarize = |report: &dartq_semantic::FileReport| {
        report
            .diagnostics
            .iter()
            .map(|row| (row.code.name, row.range.offset))
            .collect::<Vec<_>>()
    };
    let table_summary = summarize(&table_rows);
    assert_eq!(table_summary, summarize(&inline_rows));
    // The closure is load-bearing in this fixture: `Base` is spellable in
    // the use unit only through the closure, and the `l is Base` arm
    // chain must dead the else branch under BOTH paths.
    let dead = table_summary
        .iter()
        .filter(|(code, _)| *code == "dead_code")
        .count();
    assert!(dead > 0, "rows: {table_summary:?}");
}
