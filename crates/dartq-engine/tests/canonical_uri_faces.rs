//! The canonical-URI spelling faces (`FileState.uriStr` =
//! `rewriteToCanonicalUri`): a `part`'s missing-target row reports the
//! canonical spelling — `package:` inside a package, `file://` outside —
//! while import/export rows keep the written literal.
//!
//! The package fixture mirrors the conformance chunk's synthesized shape
//! exactly (pubspec `name: test`, `.dart_tool/package_config.json` with the
//! `test` entry at `packageUri: "lib/"`), so the pins hold in the chunked
//! differential runs where the oracle spells `package:test/...`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use dartq_engine::{Engine, EngineConfig, memory_store};
use dartq_package::PackageTable;
use dartq_pub::{LockFile, PackageConfig, Pubspec, build_graph};
use dartq_resolution::{
    DirectiveKind, PackageTable as UriPackageTable, ResolvedTarget, UriResolver,
    compose_canonical_uri,
};
use dartq_sdk::SdkSnapshot;
use dartq_source::FileTable;

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// The conformance chunk's synthesized package root: pubspec `name: test`,
/// the package config the materializer writes (`test` at `rootUri: "../"`,
/// `packageUri: "lib/"`), a synthetic `dart:core`-only SDK, and the given
/// files. Returns the assembled faces the CLI ships (canonicalized root,
/// parsed on-disk config, identity table).
fn chunk_fixture(tag: &str, files: &[(&str, &str)]) -> ChunkFixture {
    let root = std::env::temp_dir().join(format!(
        "dartq-engine-canonical-uri-{tag}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&root);
    let write = |relative: &str, content: &str| {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
        std::fs::write(path, content).expect("file");
    };
    write("pubspec.yaml", "name: test\n");
    write(
        ".dart_tool/package_config.json",
        r#"{"configVersion": 2, "packages": [{"languageVersion": "3.13", "name": "test", "packageUri": "lib/", "rootUri": "../"}]}"#,
    );
    write("sdk/version", "3.13.1\n");
    write(
        "sdk/lib/libraries.json",
        r#"{"vm": {"libraries": {"core": {"uri": "core/core.dart"}, "math": {"uri": "math/math.dart"}}}}"#,
    );
    write(
        "sdk/lib/_internal/allowed_experiments.json",
        r#"{"version": 1, "experimentSets": {"sdkExperiments": []}, "sdk": {"default": {"experimentSet": "sdkExperiments"}}, "packages": {}}"#,
    );
    for (relative, content) in files {
        write(relative, content);
    }

    let root = root.canonicalize().expect("fixture root resolves");
    let config_dir = root.join(".dart_tool");
    // The load path reads the file the runner synthesized, exactly as the
    // CLI assembly does.
    let config = Arc::new(
        PackageConfig::parse(
            &std::fs::read_to_string(config_dir.join("package_config.json")).expect("config"),
        )
        .expect("valid config"),
    );
    let lock = LockFile::parse("packages: {}\nsdks:\n  dart: \">=3.5.0 <4.0.0\"\n").expect("lock");
    let pubspec = Pubspec::parse("name: test\n").expect("pubspec");
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
    ChunkFixture {
        root,
        config_dir,
        config,
        packages,
    }
}

struct ChunkFixture {
    root: PathBuf,
    config_dir: PathBuf,
    config: Arc<PackageConfig>,
    packages: PackageTable,
}

impl ChunkFixture {
    fn engine(&self) -> Engine {
        Engine::new(EngineConfig {
            store: memory_store(),
            sdk: SdkSnapshot::load(&self.root.join("sdk"), None).expect("synthetic SDK loads"),
            packages: self.packages.clone(),
            pub_config: Arc::clone(&self.config),
            config_dir: self.config_dir.clone(),
            context_roots: Vec::new(),
            semdiag: Some(dartq_engine::semdiag::diagnoser()),
            target: dartq_engine::DEFAULT_TARGET.to_owned(),
            declared_variables: rustc_hash::FxHashMap::default(),
            experiments: Vec::new(),
        })
        .expect("engine")
    }

    /// The message of the one row of `code` reported for `main.dart`'s URI
    /// literal.
    fn row_message(&self, engine: &mut Engine, code: &str) -> String {
        let diagnostics = engine
            .analyze(
                &[],
                &mut |_| {},
                &dartq_engine::CancellationToken::default(),
            )
            .expect("analyze completes");
        let path = self.root.join("lib/main.dart");
        let rows: Vec<_> = diagnostics
            .iter()
            .filter(|d| {
                d.code.name == code
                    && engine
                        .file_table()
                        .path(d.file)
                        .is_some_and(|p: &Path| p == path)
            })
            .collect();
        assert_eq!(rows.len(), 1, "{code} rows: {diagnostics:?}");
        rows[0].message.to_string()
    }
}

/// Pin 1 — a part's missing target inside a package spells the canonical
/// `package:` form: the oracle's `includedFile.uriStr`
/// (library_analyzer.dart:1030-1046) runs the URI through
/// `rewriteToCanonicalUri`, so the row never prints the raw `file://` path
/// when the target sits under a package lib dir.
#[test]
fn part_missing_target_spells_package_uri() {
    let fixture = chunk_fixture(
        "part-missing",
        &[("lib/main.dart", "part 'sub/gone.dart';\n")],
    );
    let mut engine = fixture.engine();
    engine.set_content(
        &fixture.root.join("lib/main.dart"),
        b"part 'sub/gone.dart';\n",
    );
    assert_eq!(
        fixture.row_message(&mut engine, "uri_does_not_exist"),
        "Target of URI doesn't exist: 'package:test/sub/gone.dart'.",
        "the part row spells the package URI"
    );
}

/// Pin 2 — the bare floor (no package coverage, the degraded assembly's
/// `PackageTable::basic` shape) keeps the `file://` face: outside every
/// package lib dir the canonical spelling IS the `file://` URI.
#[test]
fn bare_fixture_spells_file_uri() {
    let root = std::env::temp_dir().join(format!(
        "dartq-engine-canonical-uri-bare-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&root);
    let write = |relative: &str, content: &str| {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
        std::fs::write(path, content).expect("file");
    };
    write("sdk/version", "3.13.1\n");
    write(
        "sdk/lib/libraries.json",
        r#"{"vm": {"libraries": {"core": {"uri": "core/core.dart"}}}}"#,
    );
    write(
        "sdk/lib/_internal/allowed_experiments.json",
        r#"{"version": 1, "experimentSets": {"sdkExperiments": []}, "sdk": {"default": {"experimentSet": "sdkExperiments"}}, "packages": {}}"#,
    );
    write("main.dart", "part 'gone.dart';\n");
    let root = root.canonicalize().expect("fixture root resolves");
    // The degraded floor's faces: the bare identity table, the empty
    // package resolution, the (absent) config directory.
    let packages = PackageTable::basic(&root, None).expect("bare table");
    let config = Arc::new(
        PackageConfig::parse(r#"{"configVersion": 2, "packages": []}"#).expect("empty config"),
    );
    let mut engine = Engine::new(EngineConfig {
        store: memory_store(),
        sdk: SdkSnapshot::load(&root.join("sdk"), None).expect("synthetic SDK loads"),
        packages,
        pub_config: config,
        config_dir: root.join(".dart_tool"),
        context_roots: Vec::new(),
        semdiag: Some(dartq_engine::semdiag::diagnoser()),
        target: dartq_engine::DEFAULT_TARGET.to_owned(),
        declared_variables: rustc_hash::FxHashMap::default(),
        experiments: Vec::new(),
    })
    .expect("engine");
    engine.set_content(&root.join("main.dart"), b"part 'gone.dart';\n");
    let diagnostics = engine
        .analyze(
            &[],
            &mut |_| {},
            &dartq_engine::CancellationToken::default(),
        )
        .expect("analyze completes");
    let rows: Vec<_> = diagnostics
        .iter()
        .filter(|d| d.code.name == "uri_does_not_exist")
        .collect();
    assert_eq!(rows.len(), 1, "{:?}", diagnostics);
    assert_eq!(
        rows[0].message.to_string(),
        format!(
            "Target of URI doesn't exist: 'file://{}/gone.dart'.",
            root.display()
        ),
        "the bare floor's canonical spelling is the file URI"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// Pin 3 — the generated-part face spells canonical too: a `part` coded
/// `uri_has_not_been_generated` (a `.g.dart` suffix under a package lib
/// dir) reports the `package:` spelling, the same `includedFile.uriStr`
/// arm as the `uri_does_not_exist` face.
#[test]
fn generated_part_face_spells_package_uri() {
    let fixture = chunk_fixture("part-generated", &[("lib/main.dart", "part 'a.g.dart';\n")]);
    let mut engine = fixture.engine();
    engine.set_content(&fixture.root.join("lib/main.dart"), b"part 'a.g.dart';\n");
    assert_eq!(
        fixture.row_message(&mut engine, "uri_has_not_been_generated"),
        "Target of URI hasn't been generated: 'package:test/a.g.dart'.",
        "the generated-part row spells the package URI"
    );
}

/// Pin 4 — the load path (state.rs `UriPackageTable::new(pub_config,
/// config_dir)`) resolves `package:test` from the chunk's synthesized
/// fixture bytes: the entry lands in the table, the canonical composition
/// spells every lib file through it, and paths outside the lib dir stay
/// `file://`.
#[test]
fn uri_package_table_resolves_the_synthesized_chunk_shape() {
    let fixture = chunk_fixture("load-path", &[("lib/a.dart", "void a() {}\n")]);
    let table = UriPackageTable::new(&fixture.config, &fixture.config_dir)
        .expect("the synthesized config backs the URI table");
    assert_eq!(
        table.lib_dir("test"),
        Some(fixture.root.join("lib").as_path()),
        "the test entry resolves to the package lib dir"
    );
    assert_eq!(
        &*compose_canonical_uri(&table, &fixture.root.join("lib/a.dart")),
        "package:test/a.dart",
    );
    assert_eq!(
        &*compose_canonical_uri(&table, &fixture.root.join("lib/sub/gone.dart")),
        "package:test/sub/gone.dart",
    );
    assert_eq!(
        &*compose_canonical_uri(&table, &fixture.root.join("pubspec.yaml")),
        format!("file://{}/pubspec.yaml", fixture.root.display()),
        "outside the lib dir the canonical spelling is the file URI"
    );
}

/// The two-case chunk floor's faces: the bare analyzed dir holds two
/// same-named sibling roots (`0` and `1`, each pubspec `name: test` with its
/// own `.dart_tool/package_config.json`). The merged resolution keeps only
/// the first `test` entry — one identity per name is a reader invariant —
/// so the assembly threads BOTH discovered roots through
/// `EngineConfig::context_roots`, and the URI table builds one context
/// universe per root. Returns the canonical chunk root and the assembled
/// config the CLI ships.
fn two_case_chunk_fixture() -> (PathBuf, Arc<PackageConfig>, PathBuf) {
    let chunk = std::env::temp_dir().join(format!(
        "dartq-engine-canonical-uri-chunk-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&chunk);
    for index in ["0", "1"] {
        let case = chunk.join(index);
        std::fs::create_dir_all(case.join("lib")).expect("case lib");
        std::fs::create_dir_all(case.join(".dart_tool")).expect("case dart_tool");
        std::fs::write(case.join("pubspec.yaml"), "name: test\n").expect("case pubspec");
        std::fs::write(case.join("lib/a.dart"), "void a() {}\n").expect("case source");
        std::fs::write(case.join("lib/main.dart"), "import 'a.dart';\n").expect("case source");
        std::fs::write(
            case.join(".dart_tool/package_config.json"),
            r#"{"configVersion": 2, "packages": [{"languageVersion": "3.13", "name": "test", "packageUri": "lib/", "rootUri": "../"}]}"#,
        )
        .expect("case config");
    }
    let chunk = chunk.canonicalize().expect("chunk root resolves");
    // The merged floor config: only the alphabetically first `test` entry
    // survives the name dedup — the assembly's exact output shape.
    let config = Arc::new(
        PackageConfig::parse(
            r#"{"configVersion": 2, "packages": [{"languageVersion": "3.13", "name": "test", "packageUri": "lib/", "rootUri": "../0"}]}"#,
        )
        .expect("merged config"),
    );
    let config_dir = chunk.join(".dart_tool");
    (chunk, config, config_dir)
}

/// Pin 5 — per-context universes: both same-named sibling roots spell
/// `package:test/...` (the probe's failing shape), each case's
/// `package:test/a.dart` resolves onto ITS OWN file, and a file outside
/// every context falls back to the flat merged scan (the floor's `file://`
/// face). Before the per-context split, root `1` spelled `file://` and
/// resolved onto root `0`'s file — the double collapse.
#[test]
fn per_context_universes_spell_and_resolve_each_sibling_root() {
    let (chunk, config, config_dir) = two_case_chunk_fixture();
    let roots = [chunk.join("0"), chunk.join("1")];
    let table = UriPackageTable::new_with_context_roots(&config, &config_dir, &roots)
        .expect("the floor config backs the URI table");

    // Spelling: each sibling's lib file spells through its own context —
    // both `package:test/...` (the oracle's per-context `pathToUri`).
    assert_eq!(
        &*compose_canonical_uri(&table, &roots[0].join("lib/a.dart")),
        "package:test/a.dart",
        "case 0 spells its package URI"
    );
    assert_eq!(
        &*compose_canonical_uri(&table, &roots[1].join("lib/a.dart")),
        "package:test/a.dart",
        "case 1 spells the same package URI through its own context"
    );

    // Resolution: `package:test/a.dart` from a case-N file lands on case
    // N's own target — the sibling roots never cross-resolve.
    let sdk_root = {
        let sdk = chunk.join("sdk");
        std::fs::create_dir_all(sdk.join("lib/_internal")).expect("sdk dirs");
        std::fs::write(sdk.join("version"), "3.13.1\n").expect("sdk version");
        std::fs::write(
            sdk.join("lib/libraries.json"),
            r#"{"vm": {"libraries": {"core": {"uri": "core/core.dart"}}}}"#,
        )
        .expect("sdk libraries");
        std::fs::write(
            sdk.join("lib/_internal/allowed_experiments.json"),
            r#"{"version": 1, "experimentSets": {"sdkExperiments": []}, "sdk": {"default": {"experimentSet": "sdkExperiments"}}, "packages": {}}"#,
        )
        .expect("sdk experiments");
        sdk
    };
    let snapshot = SdkSnapshot::load(&sdk_root, None).expect("synthetic SDK loads");
    let declared = rustc_hash::FxHashMap::default();
    let mut files = FileTable::new();
    let case0 = files.intern(&roots[0].join("lib/main.dart"));
    let case1 = files.intern(&roots[1].join("lib/main.dart"));
    let mut resolver = UriResolver::new(&table, &snapshot, &declared, &mut files, &|_| false);
    let from_case0 = resolver.resolve(
        case0,
        Some("package:test/a.dart"),
        DirectiveKind::ImportExport,
    );
    let from_case1 = resolver.resolve(
        case1,
        Some("package:test/a.dart"),
        DirectiveKind::ImportExport,
    );
    let target_of = |resolution: &dartq_resolution::UriResolution| match resolution.target {
        ResolvedTarget::Resolved(id) => resolver
            .file_table()
            .path(id)
            .expect("target path")
            .to_path_buf(),
        other => panic!("expected a resolved target, got {other:?}: {resolution:?}"),
    };
    assert_eq!(
        target_of(&from_case0),
        roots[0].join("lib/a.dart"),
        "case 0's package:test resolves onto case 0's file"
    );
    assert_eq!(
        target_of(&from_case1),
        roots[1].join("lib/a.dart"),
        "case 1's package:test resolves onto case 1's own file — not case 0's"
    );
    assert!(from_case0.code.is_none() && from_case1.code.is_none());

    // Flat fallback: a file under no context root keeps the flat merged
    // spelling (`file://` outside the merged lib dirs).
    assert_eq!(
        &*compose_canonical_uri(&table, &chunk.join("loose.dart")),
        format!("file://{}/loose.dart", chunk.display()),
        "a no-context file falls back to the flat scan"
    );

    let _ = std::fs::remove_dir_all(&chunk);
}
