//! `dartq impact` — the PRD §15.4 impact face: what a change to one file
//! would affect, over the analyzed workspace.
//!
//! The run drives the same workspace assembly as the other engine faces
//! ([`crate::open_engine`], discovery, one analyze pass — the
//! read-after-write the query faces share), then assembles the impact
//! substrate from the engine's public graph faces and the dartq-index
//! capture:
//!
//! * the **import closure** reads the engine's library import CSR (node =
//!   engine [`dartq_engine::LibraryId`] index);
//! * the **test graph** joins the CSR import edges with the
//!   defining-file basenames (dartq-index's derivation: imports primary,
//!   the `*_test.dart` convention fallback);
//! * the **file snapshot** folds the seed library's member files — blake3
//!   over the member hashes, the library-face declaration names from the
//!   dartq-index capture pass.
//!
//! One-shot posture: a CLI run holds no baseline, so the seed enters the
//! query as a **hypothetical** API-level change ([`LibraryChange::hypothetical`])
//! — the report answers "what depends on this file's API surface". The
//! daemon passes real before/after diffs through the same query.
//!
//! Output is the §15.7 JSON contract (`schema_version: 1`, stable path
//! labels; the v1 labels are the canonical file paths — package-form URIs
//! land with the daemon's URI-aware substrate).

use std::io::{self, BufWriter, Write};
use std::path::PathBuf;

use clap::Args;

use dartq_ai::impact::{self as query, content_hash, FileSnapshot, LibraryChange};
use dartq_engine::{Cancelled, CancellationToken, Engine, EngineEvent, LibraryId};
use dartq_index::{TestGraph, capture};
use dartq_language::{DEFAULT_LANGUAGE_VERSION, FeatureSet};
use dartq_parser::ParserFeatures;
use dartq_scanner::{ScanConfig, scan};
use dartq_source::{FileId, StringId, StringTable};

use crate::{exit, open_engine};

/// The scan configuration of every non-version-gated capture face (the
/// workspace default: triple shift and augmentations on).
const SCAN_CONFIG: ScanConfig = ScanConfig {
    enable_triple_shift: true,
    enable_augmentations: true,
};

/// The stable label and the test-detection basename of one library, keyed
/// by the engine library id (`None` on a dead slot).
type Ident = (String, String);
type IdentTable = Vec<Option<Ident>>;

#[derive(Args)]
pub(crate) struct ImpactArgs {
    /// The changed file to assess; must be an analyzed `.dart` file of the
    /// workspace.
    pub(crate) path: PathBuf,
    /// Package directory holding pubspec.yaml and
    /// `.dart_tool/package_config.json`; defaults to the current directory.
    #[arg(default_value = ".")]
    pub(crate) dir: PathBuf,
    /// Dart SDK root; defaults to `$DART_SDK`.
    pub(crate) sdk: Option<PathBuf>,
}

/// Crash tier like the other engine faces: a panic aborts with exit 4 and
/// stdout kept empty, because [`run`] writes only after the query fully
/// succeeded.
pub(crate) fn run_entry(args: &ImpactArgs) -> u8 {
    let outcome =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run(args, &mut BufWriter::new(io::stdout().lock()))
        }));
    let Ok(code) = outcome else {
        eprintln!("dartq: internal error: analysis panicked");
        return exit::INTERNAL_ERROR;
    };
    code
}

/// The stable label and the test-detection basename of every live library,
/// indexed by the engine library id (dead slots hold `None`).
fn library_identities(engine: &Engine, libraries: &[(LibraryId, Vec<FileId>)]) -> IdentTable {
    let mut idents: IdentTable = vec![None; libraries.len()];
    for (library, files) in libraries {
        let Some(path) = files.first().and_then(|&file| engine.file_table().path(file)) else {
            continue;
        };
        idents[library.index() as usize] = Some((
            path.display().to_string(),
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
        ));
    }
    idents
}

/// The folded surface of one library: blake3 over the member hashes, the
/// library-face declaration names from the capture pass (parse-surface
/// capture proceeds through scan diagnostics — the under-report bias; this
/// face produces no diagnostics of its own).
fn seed_snapshot(
    engine: &Engine,
    members: &[FileId],
    strings: &StringTable,
) -> FileSnapshot {
    let mut names: Vec<StringId> = Vec::new();
    let mut folded = Vec::with_capacity(members.len() * 32);
    for &file in members {
        let bytes = engine.content(file).expect("resident member");
        folded.extend_from_slice(&content_hash(bytes));
        let text = std::str::from_utf8(bytes).expect("engine content is UTF-8");
        let scanned = scan(text, SCAN_CONFIG, file);
        let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(
            DEFAULT_LANGUAGE_VERSION,
        ));
        let captured = capture(file, text, &scanned.tokens, features, strings);
        names.extend(
            captured
                .index
                .decls()
                .iter()
                .filter(|row| row.is_library_face())
                .map(|row| row.name),
        );
    }
    names.sort_unstable();
    names.dedup();
    FileSnapshot {
        content_hash: content_hash(&folded),
        top_level_names: names.into_boxed_slice(),
    }
}

fn run(args: &ImpactArgs, out: &mut impl Write) -> u8 {
    let (mut engine, _dir) = match open_engine(&args.dir, args.sdk.as_deref()) {
        Ok(faces) => faces,
        Err(code) => return code,
    };
    if let Err(error) = engine.discover_workspace() {
        eprintln!("dartq: internal error: discovery failed: {error}");
        return exit::INTERNAL_ERROR;
    }
    // The read-after-write: the import graph and the library table reflect
    // the analyzed state only after this pass.
    let mut on_event = |_: EngineEvent| {};
    if let Err(Cancelled) = engine.analyze(&[], &mut on_event, &CancellationToken::default()) {
        eprintln!("dartq: internal error: analysis cancelled");
        return exit::INTERNAL_ERROR;
    }

    // The seed: the target file's owning library.
    let Ok(target) = args.path.canonicalize() else {
        eprintln!("error: Directory or file doesn't exist: {}", args.path.display());
        return exit::USAGE;
    };
    let Some(seed_file) = engine.file_table().try_file_id(&target) else {
        eprintln!("error: not an analyzed .dart file: {}", target.display());
        return exit::USAGE;
    };
    let Some(seed_library) = engine.library_of(seed_file) else {
        eprintln!("error: not an analyzed .dart file: {}", target.display());
        return exit::USAGE;
    };

    // Library identities and the seed's member list.
    let csr = engine.library_import_graph();
    let count = csr.nodes();
    let libraries = engine.libraries();
    let seed_library_index = seed_library.index() as usize;
    let members = libraries
        .iter()
        .find(|(library, _)| library.index() as usize == seed_library_index)
        .map(|(_, files)| files.clone())
        .expect("live seed library");
    let idents = library_identities(&engine, &libraries);

    let strings = StringTable::new();
    let current = seed_snapshot(&engine, &members, &strings);

    // The substrate over the engine's faces, then the query.
    let neighbors = |library: u32| -> &[u32] {
        let start = csr.offsets()[library as usize] as usize;
        let end = csr.offsets()[library as usize + 1] as usize;
        &csr.edges()[start..end]
    };
    let tests = TestGraph::build(count, neighbors, |library| {
        idents[library as usize]
            .as_ref()
            .map(|(_, basename)| basename.as_str())
    });
    let change = LibraryChange::hypothetical(seed_library.index(), current);
    let report = query::impact(
        count,
        neighbors,
        |library| -> &str {
            &idents[library as usize]
                .as_ref()
                .expect("live library")
                .0
        },
        &tests,
        &strings,
        &[change],
    );

    let written = out
        .write_all(serde_json::to_string(&report).expect("ImpactReport serializes").as_bytes())
        .and_then(|()| out.write_all(b"\n"))
        .and_then(|()| out.flush());
    if let Err(error) = written {
        eprintln!("dartq: internal error: writing the impact report failed: {error}");
        return exit::INTERNAL_ERROR;
    }
    exit::CLEAN
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// The fixture shape of the analyze/query faces' tests: a real package
    /// directory with a stub SDK (`sdk/lib/libraries.json` is the only file
    /// [`dartq_sdk::SdkSnapshot::load`] needs), a lib pair, and the
    /// convention-placed test that reaches the lib through its package URI.
    fn fixture(tag: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("dq_cli_impact_{tag}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let write = |relative: &str, content: &str| {
            let path = root.join(relative);
            fs::create_dir_all(path.parent().expect("parent")).expect("dir");
            fs::write(path, content).expect("file");
        };
        write(
            "pubspec.yaml",
            "name: fixture\nversion: 0.1.0\nenvironment:\n  sdk: ^3.5.0\n",
        );
        write(
            "pubspec.lock",
            "packages: {}\nsdks:\n  dart: \">=3.5.0 <4.0.0\"\n",
        );
        write(
            ".dart_tool/package_config.json",
            r#"{"configVersion": 2, "packages": [{"name": "fixture", "rootUri": "../", "languageVersion": "3.5"}], "generator": "pub"}"#,
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
        write("lib/a.dart", "class Foo {}\nvoid foo() {}\n");
        write("lib/b.dart", "import 'a.dart';\nvoid use() {\n  Foo? f = Foo();\n  foo();\n}\n");
        write(
            "test/a_test.dart",
            "import 'package:fixture/a.dart';\nimport 'package:test/test.dart';\nvoid main() {\n  test('hits foo', () {\n    foo();\n  });\n}\n",
        );
        root
    }

    /// End to end: the hypothetical change of `lib/a.dart` names its face
    /// symbols, tags the direct importers (`b.dart`, the test) semantic,
    /// and lands the test file in AffectedTests.
    #[test]
    fn impact_end_to_end() {
        let root = fixture("e2e");
        let a = root.canonicalize().expect("root").join("lib/a.dart");
        let args = ImpactArgs {
            path: a.clone(),
            dir: root.clone(),
            sdk: Some(root.join("sdk")),
        };
        let mut out: Vec<u8> = Vec::new();
        let code = run(&args, &mut out);
        assert_eq!(code, exit::CLEAN);

        let json: serde_json::Value =
            serde_json::from_str(&String::from_utf8(out).expect("UTF-8")).expect("one object");
        assert_eq!(json["schema_version"], 1);
        let symbols: Vec<&str> = json["changed"]
            .as_array()
            .expect("changed")
            .iter()
            .map(|row| row["symbol"].as_str().expect("symbol"))
            .collect();
        assert_eq!(
            symbols,
            vec![
                format!("{}|Foo", a.display()),
                format!("{}|foo", a.display()),
            ],
            "every face name, ASCII-sorted, hypothetical signature flags"
        );
        assert!(
            json["changed"]
                .as_array()
                .expect("changed")
                .iter()
                .all(|row| row["signature_changed"] == true),
        );

        let b = root.canonicalize().expect("root").join("lib/b.dart");
        let test_path = root.canonicalize().expect("root").join("test/a_test.dart");
        let affected: Vec<(String, &str)> = json["affected"]
            .as_array()
            .expect("affected")
            .iter()
            .map(|row| {
                (
                    row["path"].as_str().expect("path").to_string(),
                    row["impact"].as_str().expect("impact"),
                )
            })
            .collect();
        assert_eq!(
            affected,
            vec![
                (b.display().to_string(), "semantic"),
                (test_path.display().to_string(), "semantic"),
            ],
            "both import a directly; the import of the test resolves through the package URI"
        );
        let tests: Vec<String> = json["affected_tests"]
            .as_array()
            .expect("tests")
            .iter()
            .map(|row| row["path"].as_str().expect("path").to_string())
            .collect();
        assert_eq!(tests, vec![test_path.display().to_string()]);
        assert_eq!(
            json["affected_environments"],
            serde_json::json!([{ "label": "default", "compatible": true }]),
        );

        fs::remove_dir_all(&root).ok();
    }

    /// Usage tier: a path outside the analyzed set.
    #[test]
    fn unanalyzed_path_is_usage() {
        let root = fixture("usage");
        let args = ImpactArgs {
            path: root.join("lib/missing.dart"),
            dir: root.clone(),
            sdk: Some(root.join("sdk")),
        };
        assert_eq!(run(&args, &mut Vec::new()), exit::USAGE);
        fs::remove_dir_all(&root).ok();
    }
}
