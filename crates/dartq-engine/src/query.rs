//! The `query symbol` face (PRD §20.3; index-design-spec §8–§10): the
//! workspace symbol index answered from the engine's maintained index
//! state — the per-file captures (declaration surface + raw sites) and the
//! per-library reference surfaces analyze keeps current at the
//! `LibraryAnalyzed` point.
//!
//! # Freshness contract
//!
//! A query is a pure read of the state the latest [`Engine::analyze`]
//! produced: callers run analyze first (the read-after-write), and a query
//! never applies the overlay, re-analyzes, or otherwise mutates derived
//! state — the only side effect is interning the queried name. Querying
//! before the first analyze answers an empty set. Reference surfaces track
//! the completed-library partition of the latest analyze exactly (a
//! cancelled analyze leaves the not-yet-recomputed libraries on their
//! previous surfaces, the same partition their cached diagnostic results
//! keep).
//!
//! # Cost shape
//!
//! The answer assembles in O(rows of the answer): declaration rows come
//! straight from the maintained captures (ascending file — the deterministic
//! order `SymbolIndex` prescribed), reference rows from the maintained
//! surfaces (ascending library). No scan, no parse, no namespace rebuild —
//! the whole-workspace recapture this face used to run per query died with
//! the maintained index (the audit's query-path multiplication).

use dartq_index::{DeclRow, RefRow};
use dartq_source::{FileId, SourceRange};

use crate::state::EngineState;

pub use dartq_index::{DeclKind, RefKind};

/// The `query symbol` answer (PRD §20.3): declaration rows plus resolved
/// reference rows. Rows borrow the engine's interned spellings — lookups
/// run on interned ids and resolve to `&str` only at this boundary; nothing
/// clones per row.
#[derive(Debug, Clone)]
pub struct SymbolAnswer<'a> {
    /// Every declaration named by the query, ascending by file.
    pub decls: Vec<DeclHit<'a>>,
    /// Every resolved reference to those declarations, grouped by target
    /// file (ascending), offset-ascending within a target.
    pub refs: Vec<RefHit<'a>>,
}

/// One declaration hit of [`SymbolAnswer`].
#[derive(Clone, Copy, Debug)]
pub struct DeclHit<'a> {
    /// The declaring file.
    pub file: FileId,
    /// The declaration family.
    pub kind: DeclKind,
    /// The declared name (setters under their `name=` lookup spelling).
    pub name: &'a str,
    /// The enclosing container's name; [`None`] for top-level rows and
    /// unnamed-extension members.
    pub container: Option<&'a str>,
    /// The name token's span (UTF-16, the engine's external convention).
    pub range: SourceRange,
}

/// One resolved reference hit of [`SymbolAnswer`].
#[derive(Clone, Copy, Debug)]
pub struct RefHit<'a> {
    /// The referenced declaration's file.
    pub target_file: FileId,
    /// The use site's file — the library member whose capture recorded the
    /// row: a library spans its defining unit plus parts, so the target
    /// pair alone cannot locate the use (dartq-index `RefRow::file`).
    pub file: FileId,
    /// The relation kind of the use.
    pub kind: RefKind,
    /// The use is explicitly qualified (`p.x`) and cannot be shadowed by a
    /// local declaration.
    pub qualified: bool,
    /// The import prefix spelling that armed the resolution (`p` in
    /// `p.fa`) — the row borrows the interned spelling like
    /// [`DeclHit::name`] (§15.7: no StringId across the boundary); the
    /// JSON face assembles the §15.8 qualified-name column as
    /// `<qualifier>.<queried name>`. [`None`] for bare uses.
    pub qualified_name: Option<&'a str>,
    /// The use site's span (UTF-16).
    pub range: SourceRange,
}

/// Answers a by-name symbol query over the maintained index state (module
/// docs for the freshness and cost contracts).
pub(crate) fn symbols<'a>(state: &'a EngineState, name: &str) -> SymbolAnswer<'a> {
    // The queried name interns once; every lookup below runs on the id.
    let id = state.strings.intern(name);

    // Declaration rows from the maintained captures, ascending by file
    // (`BTreeMap` order — the deterministic order the per-query
    // `SymbolIndex` build produced). Equal-name rows of one file are
    // contiguous, so a last-wins check deduplicates the reference probes.
    let mut decls: Vec<DeclHit<'a>> = Vec::new();
    let mut targets: Vec<FileId> = Vec::new();
    for (&file, capture) in &state.captures {
        let Some(rows) = capture.index.by_name(id) else {
            continue;
        };
        for decl in rows {
            let container = (decl.container != DeclRow::NONE).then(|| {
                state
                    .strings
                    .resolve(capture.index.row(decl.container).name)
            });
            decls.push(DeclHit {
                file,
                kind: decl.kind,
                name: state.strings.resolve(decl.name),
                container,
                range: decl.range,
            });
        }
        if targets.last() != Some(&file) {
            targets.push(file);
        }
    }

    // Reference rows from the maintained surfaces, ascending by library —
    // the same order the per-query build's `SymbolIndex::references` served.
    let refs: Vec<RefHit<'a>> = targets
        .iter()
        .flat_map(|&file| {
            state
                .library_surfaces
                .values()
                .filter_map(move |surface| surface.references_of(file, id))
                .flatten()
        })
        .map(|row: &RefRow| RefHit {
            target_file: row.target_file,
            file: row.file,
            kind: row.kind,
            qualified: row.qualified,
            qualified_name: row.qualifier.map(|q| state.strings.resolve(q)),
            range: row.range,
        })
        .collect();
    SymbolAnswer { decls, refs }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use crate::{DEFAULT_TARGET, Engine, EngineConfig, memory_store};
    use dartq_package::PackageTable;
    use dartq_pub::{LockFile, PackageConfig, Pubspec, build_graph};
    use dartq_sdk::SdkSnapshot;

    const A: &str = "class Foo {\n  void meth() {}\n}\nvoid fa() {}\n";
    const B: &str =
        "import 'a.dart';\nimport 'a.dart' as p;\n\nvoid use() {\n  fa();\n  p.fa();\n}\n";

    /// A throwaway pub workspace with a synthetic VM-segment SDK — the
    /// bench fixture's shape, inlined (the engine crate has no shared
    /// test-support crate to borrow it from).
    fn workspace(tag: &str) -> PathBuf {
        static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let root = std::env::temp_dir().join(format!(
            "dartq-engine-query-{tag}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&root);
        let write = |relative: &str, content: &str| {
            let path = root.join(relative);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
            std::fs::write(path, content).expect("file");
        };
        write(
            "pubspec.yaml",
            "name: t\nversion: 0.1.0\nenvironment:\n  sdk: ^3.5.0\n",
        );
        write(
            "pubspec.lock",
            "packages: {}\nsdks:\n  dart: \">=3.5.0 <4.0.0\"\n",
        );
        write(
            ".dart_tool/package_config.json",
            r#"{"configVersion": 2, "packages": [{"name": "t", "rootUri": "../", "languageVersion": "3.5"}], "generator": "pub"}"#,
        );
        write("sdk/version", "3.13.1\n");
        write(
            "sdk/lib/libraries.json",
            r#"{"vm": {"libraries": {"core": {"uri": "core/core.dart"}}}}"#,
        );
        write(
            "sdk/lib/_internal/allowed_experiments.json",
            r#"{"version": 1, "experimentSets": {"sdkExperiments": []}, "sdk": {"default": {"experimentSet": "sdkExperiments"}}, "packages": {}}"#,
        );
        write("lib/a.dart", A);
        write("lib/b.dart", B);
        root
    }

    /// A fresh engine over the fixture workspace.
    fn engine_over(root: &std::path::Path) -> Engine {
        let config_dir = root.join(".dart_tool");
        let config = Arc::new(
            PackageConfig::parse(
                &std::fs::read_to_string(config_dir.join("package_config.json")).expect("config"),
            )
            .expect("valid config"),
        );
        let lock = LockFile::parse("packages: {}\nsdks:\n  dart: \">=3.5.0 <4.0.0\"\n")
            .expect("valid lock");
        let pubspec = Pubspec::parse("name: t\nversion: 0.1.0\nenvironment:\n  sdk: ^3.5.0\n")
            .expect("valid pubspec");
        let graph = build_graph(
            &config,
            &lock,
            &pubspec,
            &std::collections::BTreeMap::new(),
            None,
            &config_dir,
        )
        .expect("graph");
        Engine::new(EngineConfig {
            store: memory_store(),
            sdk: SdkSnapshot::load(&root.join("sdk"), None).expect("synthetic SDK loads"),
            packages: PackageTable::build(&config, &graph, &config_dir).expect("table"),
            pub_config: Arc::clone(&config),
            config_dir,
            semdiag: None,
            target: DEFAULT_TARGET.to_owned(),
            declared_variables: rustc_hash::FxHashMap::default(),
        })
        .expect("engine")
    }

    /// The §15.8 qualified-name plumbing end to end: the armed-prefix hit
    /// resolves its qualifier to the interned spelling at the query
    /// boundary (the §15.7 borrow contract), the bare twin stays `None`,
    /// and the machine face's bool bit mirrors the spelling.
    #[test]
    fn prefixed_hits_resolve_the_qualifier_spelling() {
        let root = workspace("spelling");
        let mut engine = engine_over(&root);
        engine.discover_workspace().expect("discovery");
        engine
            .analyze(&[], &mut |_| {}, &Default::default())
            .expect("analyze");

        let answer = engine.query_symbols("fa");
        let spellings: Vec<Option<&str>> = answer.refs.iter().map(|r| r.qualified_name).collect();
        assert_eq!(
            spellings,
            vec![None, Some("p")],
            "the bare call, then the prefixed twin (offset order)"
        );
        let bits: Vec<bool> = answer.refs.iter().map(|r| r.qualified).collect();
        assert_eq!(bits, vec![false, true], "the bit mirrors the spelling");

        std::fs::remove_dir_all(&root).ok();
    }
}
