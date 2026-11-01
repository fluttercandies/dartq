//! #3580 pins: the chunked-conformance floor's inline aux packages.
//!
//! The census reported 327 EXTRA `uri_does_not_exist` rows, every one on a
//! `lib/test.dart` importing a `package:` URI the oracle resolves — the
//! corpus's inline aux packages (`package:aaa/a.dart`, `package:foo/
//! src/a.dart`, …), materialized under the case root's dot-prefixed
//! `.aux/<name>/` tree and registered in that root's `package_config.json`.
//!
//! The resolution face was right (the target exists on disk, so the directive
//! carried no URI code); the fabrication came from the two faces around it:
//! discovery never descends a dot directory, and the dependency index refused
//! an in-bound package root, so the aux file was neither resident nor indexed
//! — the residue gate then reported the resolved target as missing and the
//! aux package's declarations never loaded (`undefined_identifier` cascades
//! on every member use). The dependency index now seeds package files
//! discovery cannot reach, which is the oracle's own posture (it resolves
//! such a target as a dependency library).
//!
//! Pins: silence where the oracle is silent (the aux member resolves, no URI
//! row), and the controls that a genuinely absent target still reports — an
//! unregistered package name and a registered package's missing file.

use std::fs;
use std::path::{Path, PathBuf};

use dartq_diagnostics::Diagnostic;
use dartq_engine::Engine;

use crate::analyze::run;
use crate::uri::from_path;
use crate::workspace::{engine_config, test_fixture::workspace};

const TEST_PUBSPEC: &str = "name: test\nversion: 0.1.0\nenvironment:\n  sdk: ^3.5.0\n";

/// Writes `relative` under `root`, creating parent directories.
fn write(root: &Path, relative: &str, content: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().expect("parent")).expect("dir");
    fs::write(path, content).expect("file");
}

/// The runner's materialized chunk shape: a directory of independent case
/// roots with no pubspec/config of its own (the bare floor), each root
/// carrying its own `package_config.json` and one inline aux package per
/// `aux` entry under `.aux/<name>/lib/<name>.dart` (the materializer's
/// inline-aux layout). Returns the chunk directory; the fixture's synthetic
/// SDK stays inside it (`<chunk>/sdk`).
fn chunk(tag: &str, test_source: &str, aux: &[(&str, &str)]) -> PathBuf {
    let chunk = workspace(tag, &[]);
    for relative in [
        "pubspec.yaml",
        "pubspec.lock",
        ".dart_tool/package_config.json",
    ] {
        fs::remove_file(chunk.join(relative)).expect("drop floor face");
    }
    let chunk = chunk.canonicalize().expect("chunk");
    let root = chunk.join("0");
    write(&root, "pubspec.yaml", TEST_PUBSPEC);
    write(&root, "lib/test.dart", test_source);
    let mut packages = vec![serde_json::json!({
        "name": "test",
        "rootUri": "../",
        "packageUri": "lib/",
        "languageVersion": "3.5",
    })];
    for (name, source) in aux {
        write(&root, &format!(".aux/{name}/lib/{name}.dart"), source);
        packages.push(serde_json::json!({
            "name": name,
            "rootUri": from_path(&root.join(".aux").join(name)),
            "packageUri": "lib/",
            "languageVersion": "3.5",
        }));
    }
    let config = serde_json::json!({"configVersion": 2, "packages": packages});
    write(&root, ".dart_tool/package_config.json", &config.to_string());
    chunk
}

/// One full analyze pass over the chunk through the production assembly.
fn analyze(chunk: &Path) -> Vec<Diagnostic> {
    let (config, _dir) =
        engine_config(chunk, Some(&chunk.join("sdk"))).expect("the floor assembles");
    let mut engine = Engine::new(config).expect("engine");
    run(&mut engine, std::slice::from_ref(&chunk.to_path_buf()))
        .expect("analyze")
        .diagnostics
}

/// Whether any row carries `code`.
fn has(rows: &[Diagnostic], code: &str) -> bool {
    rows.iter().any(|row| row.code.name == code)
}

/// The fixed cause: an aux package whose only source sits under the
/// dot-prefixed `.aux` root resolves like a dependency library — no
/// `uri_does_not_exist` (the oracle resolves the target) and no
/// `undefined_identifier` (the target's declarations load). Oracle witness:
/// the census family `DeprecatedMemberUse_PackageConfigWorkspaceTest` expects
/// the deprecated-member row here and no URI row.
#[test]
fn dot_dir_aux_package_resolves_without_uri_row() {
    let chunk = chunk(
        "t3580_aux",
        "import 'package:aaa/aaa.dart';\n\nvoid f() {\n  x = 0;\n}\n",
        &[("aaa", "@deprecated\nint get x => 0;\n\nset x(int _) {}\n")],
    );
    let rows = analyze(&chunk);
    let codes: Vec<&str> = rows.iter().map(|row| row.code.name).collect();
    assert!(
        !has(&rows, "uri_does_not_exist"),
        "the aux target exists, so the URI face must stay silent: {codes:?}"
    );
    assert!(
        !has(&rows, "undefined_identifier"),
        "the aux package's declarations must load through the dependency index: {codes:?}"
    );
    fs::remove_dir_all(&chunk).ok();
}

/// Control: an unregistered package name still reports — no config entry, no
/// dependency target, the oracle's own missing-URI row.
#[test]
fn missing_package_target_still_reports() {
    let chunk = chunk(
        "t3580_unregistered",
        "import 'package:zzz/missing.dart';\n",
        &[],
    );
    let rows = analyze(&chunk);
    let row = rows
        .iter()
        .find(|row| row.code.name == "uri_does_not_exist")
        .expect("the unregistered package reports");
    assert!(
        row.message.contains("package:zzz/missing.dart"),
        "the row names the written literal: {}",
        row.message
    );
    fs::remove_dir_all(&chunk).ok();
}

/// Control: a registered package's absent file still reports. The new
/// dependency seed loads the (unreadable) target, which must not mask the
/// resolver's own coded row.
#[test]
fn missing_file_in_registered_package_still_reports() {
    let chunk = chunk(
        "t3580_missing_file",
        "import 'package:aaa/gone.dart';\n",
        &[("aaa", "int get x => 0;\n")],
    );
    let rows = analyze(&chunk);
    let row = rows
        .iter()
        .find(|row| row.code.name == "uri_does_not_exist")
        .expect("the absent file reports");
    assert!(
        row.message.contains("package:aaa/gone.dart"),
        "the row names the written literal: {}",
        row.message
    );
    fs::remove_dir_all(&chunk).ok();
}
