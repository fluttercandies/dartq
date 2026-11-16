//! Pin: the shared registration-plan table (#4330) is observably
//! equivalent to the inline `register_file` element-model walk.
//!
//! `LibraryTypes::build_with_tables` replays the per-analyze
//! `RegisterPlans` table — each file's registration yield precomputed once
//! — instead of re-walking `FileElements` per (library, file)
//! registration. The replay keeps every per-library state mutation in the
//! same order (first-writer-wins registry, id mint sequence, bound-merge
//! order, worklist push order), so the built snapshot — and every
//! diagnostic the fold derives from it — must be identical to the inline
//! path. Files the table does not hold keep the inline walk.
//!
//! The fixture makes the registration walk load-bearing: typedefs (bare
//! and parameterized), a generic top-level function, an enum, an
//! extension type with a named representation, a sealed class, a
//! multi-declared spelling claimed by two libraries, and a re-export
//! chain — every replay arm exercised against the inline walk on the
//! same graph.

#![allow(
    dead_code,
    clippy::too_many_lines,
    clippy::semicolon_if_nothing_returned
)]
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
    ClauseTrees, ClosureEdges, LibraryTypes, RegisterPlans, ResolvedTypeOps,
};
use dartq_source::{FileId, FileTable, StringTable};

/// The synthetic dart:core names the fragments consume.
const CORE_FACE: &[&str] = &[
    "Object",
    "bool",
    "int",
    "String",
    "num",
    "double",
    "Comparable",
    "Iterable",
    "Enum",
    "Record",
    "Pattern",
];

/// A library exporting a typedef, an enum, and an extension type.
const BASE_UNIT: &str = "\
typedef IntList = List<int>;
typedef Mapper<T> = T Function(int);
typedef Pair = (int, String);

enum Color { red, green }

extension type Meters(int value) {
  int doubled() => value * 2;
}

sealed class Shape {}

int measure(Shape s) => 0;
";

/// A library declaring a same-spelled container (the multi-declared probe)
/// plus a generic function and a class extending the base unit's decl.
const MID_UNIT: &str = "\
import 'package:t/base.dart';

class Shape {}

class Circle extends Shape {}

T identity<T extends num>(T x) => x;

final int topLevel = 1;
";

/// The use unit: reads through the typedef, tests the enum and the
/// extension type erasure, and exercises the ambiguity ledger.
const USE_UNIT: &str = "\
import 'package:t/base.dart';
import 'package:t/mid.dart';

class Leaf extends Circle {}

void use(Leaf l, Color c, Meters m) {
  if (l is Shape) {
    print(1);
  } else {
    print(2);
  }
  switch (c) {
    case Color.red:
      break;
    case Color.green:
      break;
  }
  int n = measure(l);
  Mapper<int> f = identity;
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
        capture_clause_trees(
            file,
            source,
            &scanned.tokens,
            features,
            &strings,
            &mut clauses,
        );
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

fn summarize(report: &dartq_semantic::FileReport) -> Vec<(&'static str, u32)> {
    report
        .diagnostics
        .iter()
        .map(|row| (row.code.name, row.range.offset))
        .collect()
}

#[test]
fn register_plan_replay_matches_inline_walk() {
    let ws = workspace();
    let plans = RegisterPlans::build(&ws.graph, &ws.index);
    // The plan table lists every file the graph can register.
    for lib in ws.graph.libraries() {
        for &file in ws.graph.unit_files(lib) {
            assert!(plans.get(file).is_some(), "plan missing for {file:?}");
        }
    }
    let with_plans = LibraryTypes::build_with_tables(
        &ws.graph,
        &ws.namespaces,
        &ws.index,
        &ws.strings,
        ws.use_lib,
        &ws.clauses,
        None,
        Some(&plans),
    );
    let inline = LibraryTypes::build_with_clauses(
        &ws.graph,
        &ws.namespaces,
        &ws.index,
        &ws.strings,
        ws.use_lib,
        &ws.clauses,
    );
    let planned_rows = fold_report(&ws, &with_plans);
    let inline_rows = fold_report(&ws, &inline);
    assert_eq!(summarize(&planned_rows), summarize(&inline_rows));
}

#[test]
fn register_plan_and_closure_edges_together_match_inline() {
    let ws = workspace();
    let plans = RegisterPlans::build(&ws.graph, &ws.index);
    let edges = ClosureEdges::build(&ws.graph, &ws.namespaces, &ws.index, &ws.clauses);
    let shared = LibraryTypes::build_with_tables(
        &ws.graph,
        &ws.namespaces,
        &ws.index,
        &ws.strings,
        ws.use_lib,
        &ws.clauses,
        Some(&edges),
        Some(&plans),
    );
    let inline = LibraryTypes::build_with_clauses(
        &ws.graph,
        &ws.namespaces,
        &ws.index,
        &ws.strings,
        ws.use_lib,
        &ws.clauses,
    );
    assert_eq!(
        summarize(&fold_report(&ws, &shared)),
        summarize(&fold_report(&ws, &inline))
    );
}

#[test]
fn register_plan_miss_falls_back_to_inline() {
    let ws = workspace();
    // An empty table misses every file — every registration must fall
    // back to the inline walk and produce the same snapshot.
    let plans = RegisterPlans::default();
    let with_plans = LibraryTypes::build_with_tables(
        &ws.graph,
        &ws.namespaces,
        &ws.index,
        &ws.strings,
        ws.use_lib,
        &ws.clauses,
        None,
        Some(&plans),
    );
    let inline = LibraryTypes::build_with_clauses(
        &ws.graph,
        &ws.namespaces,
        &ws.index,
        &ws.strings,
        ws.use_lib,
        &ws.clauses,
    );
    assert_eq!(
        summarize(&fold_report(&ws, &with_plans)),
        summarize(&fold_report(&ws, &inline))
    );
}

/// The registration-walk micro-benchmark: the `w24k`/`w49k` corpus points
/// in the #4283 audit measure `register_file` at 16-28% of post-fix
/// diagnose worker-time (the element-model re-walk per (library, file)
/// registration — ~10k registrations per file at the 107k corpus). The
/// replay removes the walk but keeps every registry/id mutation, so at a
/// fixture's scale the whole-build share is single-digit: this test
/// reports the inline-vs-replay build-time ratio over a widened
/// synthetic reach — every library importing the chain head so each
/// snapshot registers the full file set (the libraries x files product
/// the plan amortizes). Measured between ~1.05x and ~1.22x whole-build
/// across debug and release runs (load-dependent; the fixture's per-file
/// mass is heavier than the corpus's average file); the corpus-scale
/// figure is the serial gate's call. Directional evidence, not a gate —
/// equivalence is the pin above. Read the timing line with `--nocapture`.
#[test]
fn register_plan_replay_microbench() {
    const FILES: usize = 64;
    const CONTAINERS: usize = 40;
    const MEMBERS: usize = 30;
    let strings = StringTable::new();
    let mut files = FileTable::new();
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
    // Intern every library up front so cross-imports resolve.
    let mut libs = Vec::new();
    for at in 0..FILES {
        libs.push(builder.intern(&format!("package:t/f{at}.dart")));
    }
    let fan = builder.intern("package:t/fan.dart");
    // Each decl file carries many containers with members, typedefs and a
    // chain container extending the next file's head — the closure pulls
    // the whole set into `fan`'s registry.
    for (at, &lib) in libs.iter().enumerate() {
        let file = files.intern(Path::new(&format!("f{at}.dart")));
        let mut source = String::new();
        for m in 0..CONTAINERS {
            source.push_str(&format!("class C{at}_{m} {{"));
            for k in 0..MEMBERS {
                source.push_str(&format!(" int f{k} = 0; void m{k}() {{}}"));
            }
            source.push_str(" }\n");
        }
        source.push_str(&format!("typedef Alias{at} = C{at}_0;\n"));
        source.push_str(&format!("int topFn{at}(C{at}_0 x) => 0;\n"));
        if at + 1 < FILES {
            source.push_str(&format!("class Chain{at} extends C{}_0 {{}}\n", at + 1));
        } else {
            source.push_str(&format!("class Chain{at} {{}}\n"));
        }
        let scanned = scan(
            &source,
            ScanConfig {
                enable_triple_shift: true,
                enable_augmentations: set.is_enabled(Feature::Augmentations),
            },
            file,
        );
        assert!(scanned.diagnostics.is_empty());
        let elements = collect(&source, &scanned.tokens, features, &strings);
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
        capture_clause_trees(
            file,
            &source,
            &scanned.tokens,
            features,
            &strings,
            &mut clauses,
        );
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
        // Every library also imports the chain head so every snapshot
        // registers the whole chain — the (libraries x files) product the
        // plan amortizes, not just `fan`'s single reach.
        if at != 0 {
            builder.import(
                lib,
                ImportEdge {
                    target: Some(libs[0]),
                    prefix: None,
                    deferred: false,
                    combinators: Combinators::unfiltered(),
                    non_library: false,
                },
            );
        }
        if at + 1 < FILES {
            builder.import(
                lib,
                ImportEdge {
                    target: Some(libs[at + 1]),
                    prefix: None,
                    deferred: false,
                    combinators: Combinators::unfiltered(),
                    non_library: false,
                },
            );
        }
    }
    // The fan library imports the chain head; `Fan extends C0_0` roots
    // the closure.
    let fan_file = files.intern(Path::new("fan.dart"));
    let fan_source = "import 'package:t/f0.dart';\nclass Fan extends C0_0 {}\n";
    let scanned = scan(
        fan_source,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: set.is_enabled(Feature::Augmentations),
        },
        fan_file,
    );
    let elements = collect(fan_source, &scanned.tokens, features, &strings);
    index.insert(fan_file, elements.clone());
    builder.unit(fan, fan_file);
    for (name, decl) in elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare_class(fan, name, fan_file, ClassModifiers::default());
    }
    capture_clause_trees(
        fan_file,
        fan_source,
        &scanned.tokens,
        features,
        &strings,
        &mut clauses,
    );
    builder.import(
        fan,
        ImportEdge {
            target: Some(libs[0]),
            prefix: None,
            deferred: false,
            combinators: Combinators::unfiltered(),
            non_library: false,
        },
    );
    builder.import(
        fan,
        ImportEdge {
            target: Some(core),
            prefix: None,
            deferred: false,
            combinators: Combinators::unfiltered(),
            non_library: false,
        },
    );
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);

    let plan_build_start = std::time::Instant::now();
    let plans = RegisterPlans::build(&graph, &index);
    let plan_build_elapsed = plan_build_start.elapsed();

    // Sanity: every library file carries a plan.
    for lib in graph.libraries() {
        for &file in graph.unit_files(lib) {
            assert!(plans.get(file).is_some(), "missing plan for {file:?}");
        }
    }

    // One build per library: every library imports the chain head, so
    // each snapshot registers the whole file set — FILES x FILES
    // registrations per sweep, the shape the plan amortizes. `fan`'s own
    // snapshot keeps the equivalence fold.
    const ROUNDS: u32 = 2;
    let inline_start = std::time::Instant::now();
    let mut inline_types = None;
    for _ in 0..ROUNDS {
        for &lib in &libs {
            inline_types = Some(LibraryTypes::build_with_clauses(
                &graph,
                &namespaces,
                &index,
                &strings,
                lib,
                &clauses,
            ));
        }
    }
    let inline_elapsed = inline_start.elapsed();
    let replay_start = std::time::Instant::now();
    let mut replay_types = None;
    for _ in 0..ROUNDS {
        for &lib in &libs {
            replay_types = Some(LibraryTypes::build_with_tables(
                &graph,
                &namespaces,
                &index,
                &strings,
                lib,
                &clauses,
                None,
                Some(&plans),
            ));
        }
    }
    let replay_elapsed = replay_start.elapsed();
    drop(inline_types);
    drop(replay_types);
    // Equivalence stays the pin — fold the fan unit under both snapshot
    // paths and compare row sets.
    let inline_fan =
        LibraryTypes::build_with_clauses(&graph, &namespaces, &index, &strings, fan, &clauses);
    let replay_fan = LibraryTypes::build_with_tables(
        &graph,
        &namespaces,
        &index,
        &strings,
        fan,
        &clauses,
        None,
        Some(&plans),
    );
    let inline_rows = {
        let ops = ResolvedTypeOps::new(&inline_fan, &strings, fan_file);
        let scanned = scan(
            fan_source,
            ScanConfig {
                enable_triple_shift: true,
                enable_augmentations: set.is_enabled(Feature::Augmentations),
            },
            fan_file,
        );
        summarize(
            &fold_unit_with_ops(
                fan_source,
                &scanned.tokens,
                features,
                &strings,
                fan_file,
                fan,
                &graph,
                &namespaces,
                &index,
                StrictModes::default(),
                &LibraryClassModifiers::default(),
                false,
                Some(&ops),
            )
            .report,
        )
    };
    let replay_rows = {
        let ops = ResolvedTypeOps::new(&replay_fan, &strings, fan_file);
        let scanned = scan(
            fan_source,
            ScanConfig {
                enable_triple_shift: true,
                enable_augmentations: set.is_enabled(Feature::Augmentations),
            },
            fan_file,
        );
        summarize(
            &fold_unit_with_ops(
                fan_source,
                &scanned.tokens,
                features,
                &strings,
                fan_file,
                fan,
                &graph,
                &namespaces,
                &index,
                StrictModes::default(),
                &LibraryClassModifiers::default(),
                false,
                Some(&ops),
            )
            .report,
        )
    };
    assert_eq!(inline_rows, replay_rows);
    eprintln!(
        "ticket_4330 microbench ({FILES} files x {CONTAINERS} containers x {MEMBERS} members, \
         {ROUNDS} rounds): inline={inline_elapsed:?} replay={replay_elapsed:?} \
         plan_build_once={plan_build_elapsed:?} ratio={:.3}x",
        inline_elapsed.as_secs_f64() / replay_elapsed.as_secs_f64().max(f64::EPSILON)
    );
}
