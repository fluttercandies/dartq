//! The dependency-package member-face pins (#2469): a workspace barrel
//! re-exporting a dependency package's containers (`export
//! 'package:dep/src/scanner/token.dart' show Token;` — the analyzer
//! package's `dart/ast/token.dart` face over `_fe_analyzer_shared`) must
//! back the full member walk for the re-exported names, not just the
//! top-level type resolution. The census mass this closes: receivers typed
//! by re-exported dependency containers answered `member_defined found=false`
//! and fired `undefined_getter`/`undefined_method` where the oracle resolves
//! the real class and stays silent (or reports only genuinely-missing
//! members).
//!
//! The fixture is a throwaway tree shaped like the dep-interface assembly
//! (see dep_index.rs): the `dep` package is hosted-classified through the
//! `pubspec.lock` entry (cache content, never workspace source) with its
//! root on disk beside the workspace, and the workspace barrel re-exports
//! the dep file's containers. The dep files are never fed as content — they
//! reach the analysis only through the on-disk load the re-export triggers.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use dartq_diagnostics::Diagnostic;
use dartq_engine::{Engine, EngineConfig, memory_store};
use dartq_package::PackageTable;
use dartq_pub::{LockFile, PackageConfig, Pubspec, build_graph};
use dartq_sdk::SdkSnapshot;

// The dependency package's base file: one supertype container the scanner
// file's classes implement, in a sibling `src/base/` file (the
// `_fe_analyzer_shared` layout's shape — the dep file's own imports sit one
// hop past the workspace barrel).
const SYNTACTIC_ENTITY: &str = "abstract class SyntacticEntity {\n  int get offset;\n}\n";

/// The dependency package's scanner file: the containers the census rows
/// name. `Token` is the abstract-implements shape (`abstract class Token
/// implements SyntacticEntity`) with instance getters; `TokenClass` is the
/// second same-package container the generalization pin reads.
const TOKEN: &str = r#"import '../base/syntactic_entity.dart';

abstract class Token implements SyntacticEntity {
  String get lexeme;
  Token? get next;
  bool get isEof;
}

class TokenClass {
  final bool isBuiltIn;

  const TokenClass(this.isBuiltIn);
}
"#;

/// The workspace barrel: an export-only library over the dep file — the
/// analyzer package's `lib/dart/ast/token.dart` shape (`library;` plus the
/// `show`-filtered re-export).
const BARREL: &str =
    "export 'package:dep/src/scanner/token.dart' show Token, TokenClass;\n";

/// The direct-import control: the same dep file reached without the barrel
/// (the tracked dep-interface corpus's shape — the known-good baseline).
const DIRECT: &str =
    "import 'package:dep/src/scanner/token.dart';\n\nvoid d(Token t) {\n  t.lexeme;\n}\n";

/// The census shape: a workspace class carries a field typed by the
/// re-exported container, and instance reads climb it. `lexeme`/`isEof`
/// resolve through the dep face (silent); `nope` is genuinely missing
/// (`undefined_getter`); `TokenClass` proves the second container.
const MAIN: &str = r#"import 'token.dart';

class N {
  final Token name;

  N(this.name);
}

void f(N n) {
  n.name.lexeme;
  n.name.isEof;
  n.name.nope;
}

void g(Token t) {
  t.lexeme;
  t.nope;
}

void h(TokenClass c) {
  c.isBuiltIn;
  c.nope;
}
"#;

/// A throwaway workspace + hosted-classified dep package, assembled through
/// the real dartq-pub/dartq-package/dartq-sdk faces (the pub_universe
/// fixture's assembly). Returns the engine and the workspace root. The
/// variant flags toggle the broken-universe shapes the corpus census
/// probes: `with_dep_entry=false` omits the dep from the package config
/// (the universe-miss shape), `dep_files=false` leaves the dep tree
/// unwritten (the resolution-resolves-but-load-fails shape), `dep_hosted=
/// false` drops the hosted lock entry (the path-classified shape the lazy
/// index skips — workspace-source posture).
fn fixture(tag: &str, with_dep_entry: bool, dep_files: bool, dep_hosted: bool) -> (Engine, PathBuf) {
    let root = std::env::temp_dir().join(format!(
        "dartq-engine-dep-member-faces-{tag}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
    ));
    let _ = std::fs::remove_dir_all(&root);
    let write = |relative: &str, content: &str| {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
        std::fs::write(path, content).expect("file");
    };
    write("ws/pubspec.yaml", "name: ws\nversion: 0.1.0\nenvironment:\n  sdk: ^3.5.0\n");
    // The hosted `dep` entry: the identity table classifies the package as
    // cache content (not workspace source) — the shape the lazy
    // dependency-interface index keys on (the dep-interface corpus's lock).
    // The path-classified variant (the SDK-checkout workspace's own pkg/*
    // tree) keys the opposite posture: workspace source the lazy index
    // never loads.
    let lock_body = if dep_hosted {
        r#"packages:
  dep:
    dependency: "direct main"
    description:
      name: dep
      sha256: "0000000000000000000000000000000000000000000000000000000000000000"
      url: "https://pub.dev"
    source: hosted
    version: "1.0.0"
sdks:
  dart: ">=3.5.0 <4.0.0"
"#
    } else {
        r#"packages:
  dep:
    dependency: "direct main"
    description:
      path: "../dep"
      relative: true
    source: path
    version: "1.0.0"
sdks:
  dart: ">=3.5.0 <4.0.0"
"#
    };
    write("ws/pubspec.lock", lock_body);
    let config_body = if with_dep_entry {
        r#"{"configVersion": 2, "packages": [{"name": "ws", "rootUri": "../", "packageUri": "lib/", "languageVersion": "3.5"}, {"name": "dep", "rootUri": "../../dep", "packageUri": "lib/", "languageVersion": "3.5"}], "generator": "pub"}"#
    } else {
        r#"{"configVersion": 2, "packages": [{"name": "ws", "rootUri": "../", "packageUri": "lib/", "languageVersion": "3.5"}], "generator": "pub"}"#
    };
    write("ws/.dart_tool/package_config.json", config_body);
    write("lib/token.dart", BARREL);
    write("lib/direct.dart", DIRECT);
    write("lib/main.dart", MAIN);
    // The dep package's files: disk-only content the re-export's resolution
    // loads (never fed as engine content).
    if dep_files {
        write("dep/pubspec.yaml", "name: dep\nversion: 1.0.0\n");
        write("dep/lib/src/base/syntactic_entity.dart", SYNTACTIC_ENTITY);
        write("dep/lib/src/scanner/token.dart", TOKEN);
    }
    // The synthetic VM-segment SDK: the throwaway minimum.
    write("sdk/version", "3.13.1\n");
    write(
        "sdk/lib/libraries.json",
        r#"{"vm": {"libraries": {"core": {"uri": "core/core.dart"}}}}"#,
    );
    write(
        "sdk/lib/_internal/allowed_experiments.json",
        r#"{"version": 1, "experimentSets": {"sdkExperiments": []}, "sdk": {"default": {"experimentSet": "sdkExperiments"}}, "packages": {}}"#,
    );

    // Canonicalize before the pub faces resolve it (the #287 spelling
    // universe — macOS /var vs /private/var).
    let root = root.canonicalize().expect("fixture root resolves");
    let ws = root.join("ws");
    let config_dir = ws.join(".dart_tool");
    let config = Arc::new(
        PackageConfig::parse(
            &std::fs::read_to_string(config_dir.join("package_config.json")).expect("config"),
        )
        .expect("valid config"),
    );
    let lock = LockFile::parse(
        &std::fs::read_to_string(ws.join("pubspec.lock")).expect("lock"),
    )
    .expect("valid lock");
    let pubspec = Pubspec::parse(&std::fs::read_to_string(ws.join("pubspec.yaml")).expect("pubspec"))
        .expect("pubspec");
    let graph = build_graph(
        &config,
        &lock,
        &pubspec,
        &std::collections::BTreeMap::new(),
        None,
        &config_dir,
    )
    .expect("graph");
    let packages = PackageTable::build(&config, &graph, &config_dir).expect("table");
    let engine = Engine::new(EngineConfig {
        store: memory_store(),
        sdk: SdkSnapshot::load(&root.join("sdk"), None).expect("synthetic SDK loads"),
        packages,
        pub_config: config,
        config_dir,
        semdiag: Some(dartq_engine::semdiag::diagnoser()),
        target: dartq_engine::DEFAULT_TARGET.to_owned(),
        declared_variables: rustc_hash::FxHashMap::default(),
        experiments: Vec::new(),
    })
    .expect("engine");
    (engine, ws)
}

static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// The code names reported for the file whose path ends with `tail`, sorted
/// and deduped (the engine's canonical order already sorts; the collect is
/// a per-file projection).
fn codes_for(diagnostics: &[Diagnostic], engine: &Engine, tail: &str) -> Vec<&'static str> {
    let mut codes: Vec<&'static str> = diagnostics
        .iter()
        .filter(|diagnostic| {
            engine
                .file_table()
                .path(diagnostic.file)
                .is_some_and(|path: &Path| path.ends_with(tail))
        })
        .map(|diagnostic| diagnostic.code.name)
        .collect();
    codes.sort_unstable();
    codes.dedup();
    codes
}

/// Feeds the workspace files and analyzes; returns the diagnostics.
fn analyze_ws(engine: &mut Engine, ws: &Path) -> Vec<Diagnostic> {
    engine.set_content(&ws.join("lib/token.dart"), BARREL.as_bytes());
    engine.set_content(&ws.join("lib/direct.dart"), DIRECT.as_bytes());
    engine.set_content(&ws.join("lib/main.dart"), MAIN.as_bytes());
    engine
        .analyze(
            &[],
            &mut |_| {},
            &dartq_engine::CancellationToken::default(),
        )
        .expect("analyze completes")
}

/// The direct-import control: a dep file's member face reached without the
/// barrel backs the member walk (the tracked dep-interface corpus's pin,
/// restated on this fixture so the barrel pins below fail at their own hop,
/// not at the base machinery).
#[test]
fn direct_dep_import_member_resolves() {
    let (mut engine, ws) = fixture("direct", true, true, true);
    let diagnostics = analyze_ws(&mut engine, &ws);
    assert!(
        !codes_for(&diagnostics, &engine, "direct.dart")
            .contains(&"undefined_getter"),
        "the direct dep import's member read must resolve: {:?}",
        codes_for(&diagnostics, &engine, "direct.dart")
    );
}

/// The census shape: instance members of a container reached through a
/// workspace barrel's `show`-filtered re-export resolve — `lexeme`/`isEof`
/// on `Token` and `isBuiltIn` on the same package's `TokenClass` stay
/// silent, while the genuinely-missing `nope` still reports per read site.
#[test]
fn barrel_reexport_member_faces_resolve() {
    let (mut engine, ws) = fixture("barrel", true, true, true);
    let diagnostics = analyze_ws(&mut engine, &ws);
    let codes = codes_for(&diagnostics, &engine, "main.dart");
    assert_eq!(
        codes,
        vec!["undefined_getter"],
        "the re-exported containers' real members must resolve and the \
         genuinely-missing member must stay the only report family"
    );
    // Exact count: one `nope` per read site (three), nothing else — the
    // over-correction guard (a suppress-everything posture would drop
    // these; a face-table-only posture would drop nothing but also never
    // fire).
    let getter_rows = diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic.code.name == "undefined_getter"
                && engine
                    .file_table()
                    .path(diagnostic.file)
                    .is_some_and(|path: &Path| path.ends_with("main.dart"))
        })
        .count();
    assert_eq!(
        getter_rows, 3,
        "each genuinely-missing member read reports once: {codes:?}"
    );
}

/// The dep files themselves stay row-silent (the index serves faces, never
/// diagnostics — the differential counts workspace files only).
#[test]
fn dep_files_stay_row_silent() {
    let (mut engine, ws) = fixture("silent", true, true, true);
    let diagnostics = analyze_ws(&mut engine, &ws);
    let dep_root = ws.parent().expect("fixture root").join("dep");
    let dep_rows: Vec<&Diagnostic> = diagnostics
        .iter()
        .filter(|diagnostic| {
            engine
                .file_table()
                .path(diagnostic.file)
                .is_some_and(|path: &Path| path.starts_with(&dep_root))
        })
        .collect();
    assert!(
        dep_rows.is_empty(),
        "dependency files must stay row-silent: {dep_rows:?}"
    );
}

/// The broken-dependency parity face: the universe lists the dep but its
/// tree is absent on disk. Resolution still lands (the interned path), the
/// dep load fails, and the barrel's re-export carries nothing — the oracle
/// verdict on this exact shape (probed: `dart analyze` over a rig with the
/// dep entry dropped from its config) is `uri_does_not_exist` on the
/// directive plus `undefined_class` at the type uses, with every member
/// read silent (the receiver's static type is the oracle's `InvalidType`).
/// The engine must neither over-report the member family (the #2442
/// residual shape) nor suppress the name family the oracle carries.
#[test]
fn broken_dep_reports_names_not_members_like_the_oracle() {
    let (mut engine, ws) = fixture("broken_files", true, false, true);
    let diagnostics = analyze_ws(&mut engine, &ws);
    assert_eq!(
        codes_for(&diagnostics, &engine, "token.dart"),
        vec!["uri_does_not_exist"],
        "the barrel's dead export reports the URI face"
    );
    for tail in ["direct.dart", "main.dart"] {
        let codes = codes_for(&diagnostics, &engine, tail);
        // direct.dart's own import spells the dead URI, so its name family
        // rides beside the URI row; main.dart reaches the name through the
        // barrel only.
        let expected: Vec<&str> = if tail == "direct.dart" {
            vec!["undefined_class", "uri_does_not_exist"]
        } else {
            vec!["undefined_class"]
        };
        assert_eq!(
            codes, expected,
            "{tail}: the unresolved container reports the name family and \
             every member read stays silent: {codes:?}"
        );
    }
}

/// The universe-miss parity face: the config carries no dep entry at all —
/// resolution misses outright. The oracle verdict on this exact shape
/// (probed: `dart analyze` over the same rig layout) is `uri_does_not_exist`
/// on the directive plus `undefined_class` at the type uses, members silent.
#[test]
fn missing_universe_entry_reports_names_not_members_like_the_oracle() {
    let (mut engine, ws) = fixture("broken_entry", false, true, true);
    let diagnostics = analyze_ws(&mut engine, &ws);
    assert_eq!(
        codes_for(&diagnostics, &engine, "token.dart"),
        vec!["uri_does_not_exist"],
        "the barrel's unresolvable export reports the URI face"
    );
    for tail in ["direct.dart", "main.dart"] {
        let codes = codes_for(&diagnostics, &engine, tail);
        // direct.dart's own import spells the dead URI, so its name family
        // rides beside the URI row; main.dart reaches the name through the
        // barrel only.
        let expected: Vec<&str> = if tail == "direct.dart" {
            vec!["undefined_class", "uri_does_not_exist"]
        } else {
            vec!["undefined_class"]
        };
        assert_eq!(
            codes, expected,
            "{tail}: the unresolved container reports the name family and \
             every member read stays silent: {codes:?}"
        );
    }
}

/// The path-classified dep: the identity table classifies it as workspace
/// source (the SDK-checkout shape, where every pkg/* package is the
/// checkout's own source), so discovery's package-root walk makes its files
/// resident with full element models — the re-exported containers carry
/// their real member faces exactly like the hosted shape.
#[test]
fn path_classified_dep_carries_member_faces() {
    let (mut engine, ws) = fixture("path_dep", true, true, false);
    // The production posture: discovery's package-root walk (every root in
    // the identity table) makes the path dep's files resident before the
    // analyze.
    engine.discover_workspace().expect("discovery");
    engine.apply_pending(&mut |_| {});
    let diagnostics = analyze_ws(&mut engine, &ws);
    let codes = codes_for(&diagnostics, &engine, "main.dart");
    assert_eq!(
        codes,
        vec!["undefined_getter"],
        "the path-classified dep's member faces resolve like the hosted \
         shape; only the genuinely-missing member reports: {codes:?}"
    );
}
