//! Case corpus loading and materialization: a `.case` file becomes one
//! standalone package root (its own `pubspec.yaml`, `analysis_options.yaml`,
//! `.dart_tool/package_config.json` and file sections), so a chunk directory
//! of such roots is one `dart analyze` invocation. Inline expectation
//! markers are stripped exactly like the official harness does before
//! analysis, which also puts the oracle output and the marker positions in
//! the same (stripped) coordinate space.
//!
//! Case-supplied environment sections are honored verbatim: a `pubspec.yaml`
//! section replaces the generated `name: test` pubspec, and an
//! `analysis_options.yaml` section replaces the generated options unless the
//! case also carries an `options:` directive — the directive is the
//! effective-options channel (the extracted corpus pairs the two, with the
//! directive carrying the merged options), so it wins when both are present.

use crate::case::{self, Case};
use crate::markers;
use crate::setup;
use blake3::Hasher;
use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};

/// Documentation-derived spec source (the official `verifyDocs` pipeline,
/// where `[!…!]` marks the expected error range); test-derived cases use
/// caret/`[diag.x]` markers instead and never get bracket stripping.
fn is_doc_sourced(case: &Case) -> bool {
    case.source.contains("messages.yaml#")
}

/// Removes the first `[!`…`!]` pair per the official `_extractSnippetData`:
/// content = before `[!` + inside + after `!]` (only the first pair, exactly
/// like upstream; a second pair stays and its parse errors are honest
/// oracle input).
fn remove_doc_range(content: &str) -> String {
    let start = match content.find("[!") {
        Some(start) => start,
        None => return content.to_string(),
    };
    let end = match content[start + 2..].find("!]") {
        Some(end) => start + 2 + end,
        None => return content.to_string(),
    };
    let mut out = String::with_capacity(content.len() - 4);
    out.push_str(&content[..start]);
    out.push_str(&content[start + 2..end]);
    out.push_str(&content[end + 2..]);
    out
}

/// The analysis input for one file section: expectation marker lines are
/// always removed (the official `removeDiagnosticExpectations`), and
/// documentation cases additionally lose their `[!…!]` range brackets.
fn analysis_content(case: &Case, content: &str) -> String {
    let stripped = markers::remove_expectations(content);
    if is_doc_sourced(case) {
        remove_doc_range(&stripped)
    } else {
        stripped
    }
}

/// The default `analysis_options.yaml` when a case carries none: the
/// official pub-package test defaults.
pub fn effective_options(options: Option<&str>) -> String {
    match options {
        Some(yaml) => yaml.to_string(),
        None => setup::default_options(None),
    }
}

/// The package_config `languageVersion` of a case.
pub fn language_version(case: &Case) -> &str {
    case.language.as_deref().unwrap_or("3.13")
}

/// A case read from the corpus: parsed content plus the blake3 of its
/// canonical bytes (the cache-key ingredient).
pub struct LoadedCase {
    pub case: Case,
    pub hash: String,
}

/// Walks a corpus tree, reads and parses every `.case` file, returns them
/// sorted by id (the traversal order of every consumer).
pub fn load_cases(dir: &Path) -> Result<Vec<LoadedCase>, String> {
    if !dir.is_dir() {
        return Err(format!(
            "case corpus {} is missing; run `conformance extract` first",
            dir.display()
        ));
    }
    let mut files = Vec::new();
    collect_case_files(dir, &mut files);
    let mut cases: Vec<LoadedCase> = Vec::with_capacity(files.len());
    let mut by_id: BTreeMap<String, PathBuf> = BTreeMap::new();
    for path in files {
        let bytes =
            std::fs::read(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        let text =
            String::from_utf8(bytes).map_err(|_| format!("{} is not UTF-8", path.display()))?;
        let parsed = case::read(&text).map_err(|e| format!("{}: {:?}", path.display(), e))?;
        if let Some(first) = by_id.insert(parsed.id.clone(), path.clone()) {
            return Err(format!(
                "duplicate case id `{}` in {} and {}",
                parsed.id,
                first.display(),
                path.display()
            ));
        }
        cases.push(LoadedCase {
            hash: blake3::hash(text.as_bytes()).to_string(),
            case: parsed,
        });
    }
    cases.sort_by(|a, b| a.case.id.cmp(&b.case.id));
    Ok(cases)
}

fn collect_case_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_case_files(&path, out);
        } else if entry.file_name().to_string_lossy().ends_with(".case") {
            out.push(path);
        }
    }
}

/// Loads the extracted corpus plus the hand-written manual corpus
/// (`conformance/manual/**`), merged in id order. On an id collision the
/// extracted case wins and the manual file is reported. A missing manual
/// directory is reported and skipped: the manual corpus is an optional
/// addition, unlike the extracted corpus whose absence is fatal.
pub fn load_corpus(cases_dir: &Path, manual_dir: Option<&Path>) -> Result<Vec<LoadedCase>, String> {
    let mut cases = load_cases(cases_dir)?;
    let Some(manual_dir) = manual_dir else {
        return Ok(cases);
    };
    if !manual_dir.is_dir() {
        warn(&format!(
            "manual corpus {} is missing; continuing without it",
            manual_dir.display()
        ));
        return Ok(cases);
    }
    let extracted: std::collections::BTreeSet<String> =
        cases.iter().map(|c| c.case.id.clone()).collect();
    let mut manual = 0usize;
    for loaded in load_cases(manual_dir)? {
        if extracted.contains(&loaded.case.id) {
            warn(&format!(
                "manual case id `{}` collides with the extracted corpus; the extracted case wins",
                loaded.case.id
            ));
            continue;
        }
        cases.push(loaded);
        manual += 1;
    }
    if manual > 0 {
        let _ = writeln!(
            std::io::stderr().lock(),
            "manual corpus: {manual} cases loaded"
        );
    }
    cases.sort_by(|a, b| a.case.id.cmp(&b.case.id));
    Ok(cases)
}

fn warn(message: &str) {
    let _ = writeln!(std::io::stderr().lock(), "warn: {message}");
}

/// A case that cannot be materialized (missing mock package, file path
/// escaping its package root). The case is recorded like a crash on the
/// oracle side, never silently skipped.
///
/// `chunk_dir` may be relative (the runner's `target/conformance/chunks`
/// spelling); it is canonicalized here so every registered package rootUri
/// is an absolute `file:` URI — a relative one is unresolvable for the
/// analyzer, which is also the shape the official harness writes.
pub fn materialize(
    cases: &[LoadedCase],
    indices: &[usize],
    chunk_dir: &Path,
    mocks_root: &Path,
) -> BTreeMap<usize, String> {
    let _ = std::fs::remove_dir_all(chunk_dir);
    std::fs::create_dir_all(chunk_dir).expect("creates chunk dir");
    let chunk_dir = std::fs::canonicalize(chunk_dir).expect("chunk dir was just created");
    let mocks_abs = std::fs::canonicalize(mocks_root).unwrap_or_else(|_| mocks_root.to_path_buf());
    let mut errors = BTreeMap::new();
    for &i in indices {
        let loaded = &cases[i];
        if let Err(e) = materialize_one(loaded, i, &chunk_dir, &mocks_abs) {
            errors.insert(i, e);
        }
    }
    errors
}

fn materialize_one(
    loaded: &LoadedCase,
    index: usize,
    chunk_dir: &Path,
    mocks_abs: &Path,
) -> Result<(), String> {
    let case = &loaded.case;
    let root = chunk_dir.join(index.to_string());
    // File sections: plain sections land relative to the package root;
    // `package:<name>/<rel>` sections are the case's inline aux packages
    // (the official harness writes the test-declared package files and
    // registers them in the same package config).
    let mut sections: BTreeMap<String, String> = BTreeMap::new();
    // name -> (relative path, analysis content)
    let mut inline_aux: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for (path, content) in &case.files {
        let content = analysis_content(case, content);
        if let Some(rest) = path.strip_prefix("package:") {
            let Some((name, rel)) = rest.split_once('/') else {
                return Err(format!("invalid package section path `{path}`"));
            };
            if name.is_empty() || name == "." || name == ".." {
                return Err(format!("invalid package section path `{path}`"));
            }
            inline_aux
                .entry(name.to_string())
                .or_default()
                .push((normalize_section_path(rel)?, content));
        } else {
            sections.insert(normalize_section_path(path)?, content);
        }
    }
    if !sections.contains_key("pubspec.yaml") {
        sections.insert("pubspec.yaml".to_string(), "name: test\n".to_string());
    }
    let language_version = language_version(case).to_string();
    // Package config entries: `test`, then every inline aux package, then
    // the mock-backed names listed by the case but not defined inline.
    // rootUri uses the official `file://` shape (`PackageConfigFileBuilder.
    // toContent`); the languageVersion pin mirrors the official harness's
    // effective value (omitted = the SDK default of this pin).
    let mut config_packages = vec![serde_json::json!({
        "name": "test",
        "rootUri": "../",
        "packageUri": "lib/",
        "languageVersion": language_version,
    })];
    for name in inline_aux.keys() {
        let aux_root = root.join(".aux").join(name);
        config_packages.push(serde_json::json!({
            "name": name,
            "rootUri": path_to_file_uri(&aux_root),
            "packageUri": "lib/",
            "languageVersion": language_version,
        }));
    }
    let mut mock_names: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    for name in case.packages.iter().chain(case.aux_packages.iter()) {
        if inline_aux.contains_key(name) {
            continue; // the case's own files win over the mock copy
        }
        let package_root = mocks_abs.join(name);
        if !package_root.is_dir() {
            return Err(format!(
                "mock package `{name}` not found under {}",
                mocks_abs.display()
            ));
        }
        mock_names.insert(name);
    }
    for name in &mock_names {
        config_packages.push(serde_json::json!({
            "name": name,
            "rootUri": path_to_file_uri(&mocks_abs.join(name)),
            "packageUri": "lib/",
            "languageVersion": "3.13",
        }));
    }
    let config = serde_json::json!({ "configVersion": 2, "packages": config_packages });

    std::fs::create_dir_all(root.join(".dart_tool"))
        .map_err(|e| format!("cannot create {}: {e}", root.join(".dart_tool").display()))?;
    for (rel, content) in &sections {
        let path = root.join(rel);
        let parent = path.parent().expect("section path has a parent");
        std::fs::create_dir_all(parent)
            .and_then(|_| std::fs::write(&path, content))
            .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    }
    for (name, files) in &inline_aux {
        for (rel, content) in files {
            let path = root.join(".aux").join(name).join(rel);
            let parent = path.parent().expect("aux file path has a parent");
            std::fs::create_dir_all(parent)
                .and_then(|_| std::fs::write(&path, content))
                .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        }
    }
    // A case-supplied `analysis_options.yaml` section is the file itself and
    // is honored verbatim when the case carries no options directive; the
    // directive (or the default) wins otherwise, which is also the extracted
    // corpus' shape: its options directives carry the merged options.
    if !sections.contains_key("analysis_options.yaml") || case.options.is_some() {
        std::fs::write(
            root.join("analysis_options.yaml"),
            effective_options(case.options.as_deref()),
        )
        .map_err(|e| format!("cannot write analysis_options.yaml: {e}"))?;
    }
    std::fs::write(
        root.join(".dart_tool").join("package_config.json"),
        config.to_string(),
    )
    .map_err(|e| format!("cannot write package_config.json: {e}"))?;
    Ok(())
}

/// The `file://` URI of an absolute path, percent-encoded per the package
/// config spec (the official builder's `rootFolder.toUri()` shape).
fn path_to_file_uri(path: &Path) -> String {
    let text = path.to_string_lossy();
    let mut out = String::from("file://");
    for (i, segment) in text.split('/').enumerate() {
        if i > 0 {
            out.push('/');
        }
        for byte in segment.bytes() {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
                out.push(byte as char);
            } else {
                out.push_str(&format!("%{byte:02X}"));
            }
        }
    }
    out
}

/// Section paths are relative to the package root; a leading `/` is the
/// case-format spelling of the root, `..` components are rejected.
fn normalize_section_path(path: &str) -> Result<String, String> {
    let trimmed = path.trim_start_matches('/');
    if trimmed.is_empty() {
        return Err(format!("empty file section path `{path}`"));
    }
    let mut parts = Vec::new();
    for part in trimmed.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                return Err(format!(
                    "file section path `{path}` escapes the package root"
                ));
            }
            _ => parts.push(part),
        }
    }
    Ok(parts.join("/"))
}

/// blake3 over the whole mock-packages tree: sorted (relative path, bytes).
pub fn mocks_tree_hash(mocks_root: &Path) -> Result<String, String> {
    let mut files = Vec::new();
    collect_files(mocks_root, Path::new(""), &mut files);
    files.sort();
    let mut hasher = Hasher::new();
    for (rel, path) in &files {
        let bytes =
            std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        hasher.update(rel.as_bytes());
        hasher.update(&[0]);
        hasher.update(&bytes);
    }
    Ok(hasher.finalize().to_string())
}

fn collect_files(dir: &Path, rel: &Path, out: &mut Vec<(String, PathBuf)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let rel = rel.join(entry.file_name());
        if path.is_dir() {
            collect_files(&path, &rel, out);
        } else if path.is_file() {
            out.push((rel.to_string_lossy().into_owned(), path));
        }
    }
}

/// The oracle cache key: blake3(case bytes + mocks tree hash + sdk_revision)
/// — a case re-analyzes exactly when any input changed.
pub fn cache_key(case_bytes_hash: &str, mocks_hash: &str, sdk_revision: &str) -> String {
    let mut hasher = Hasher::new();
    hasher.update(case_bytes_hash.as_bytes());
    hasher.update(&[0]);
    hasher.update(mocks_hash.as_bytes());
    hasher.update(&[0]);
    hasher.update(sdk_revision.as_bytes());
    hasher.finalize().to_string()
}

/// A top-level `key = "value"` entry of `conformance/pin.toml`.
pub fn pin_value(pin_text: &str, key: &str) -> Option<String> {
    for line in pin_text.lines() {
        if let Some(rest) = line.strip_prefix(key) {
            let rest = rest.trim_start_matches([' ', '\t']);
            if let Some(rest) = rest.strip_prefix('=') {
                let value = rest.trim().trim_matches('"');
                if !value.is_empty() {
                    return Some(value.to_string());
                }
            }
        }
    }
    None
}

/// `sdk_revision` from `conformance/pin.toml`.
pub fn sdk_revision(pin_path: &Path) -> Result<String, String> {
    let text =
        std::fs::read_to_string(pin_path).map_err(|e| format!("cannot read pin.toml: {e}"))?;
    pin_value(&text, "sdk_revision").ok_or_else(|| "pin.toml has no `sdk_revision`".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loaded(id: &str, files: &[(&str, &str)]) -> LoadedCase {
        LoadedCase {
            case: Case {
                id: id.into(),
                source: String::new(),
                language: None,
                packages: vec![],
                aux_packages: vec![],
                options: None,
                ignores: vec![],
                files: files
                    .iter()
                    .map(|(p, c)| (p.to_string(), c.to_string()))
                    .collect(),
            },
            hash: blake3::hash(id.as_bytes()).to_string(),
        }
    }

    #[test]
    fn section_paths_normalize_and_reject_escapes() {
        assert_eq!(
            normalize_section_path("lib/test.dart").as_deref(),
            Ok("lib/test.dart")
        );
        assert_eq!(
            normalize_section_path("/lib/test.dart").as_deref(),
            Ok("lib/test.dart")
        );
        assert_eq!(
            normalize_section_path("pubspec.yaml").as_deref(),
            Ok("pubspec.yaml")
        );
        assert!(normalize_section_path("../x.dart").is_err());
        assert!(normalize_section_path("/").is_err());
    }

    #[test]
    fn cache_key_changes_with_every_input() {
        let base = cache_key("case", "mocks", "rev");
        assert_eq!(
            base,
            cache_key("case", "mocks", "rev"),
            "stable for equal inputs"
        );
        assert_ne!(base, cache_key("case2", "mocks", "rev"));
        assert_ne!(base, cache_key("case", "mocks2", "rev"));
        assert_ne!(base, cache_key("case", "mocks", "rev2"));
    }

    #[test]
    fn mocks_hash_changes_with_content() {
        let dir = std::env::temp_dir().join(format!("conf-mocks-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("meta/lib")).expect("creates");
        std::fs::write(dir.join("meta/lib/a.dart"), "a").expect("writes");
        let h1 = mocks_tree_hash(&dir).expect("hashes");
        std::fs::write(dir.join("meta/lib/a.dart"), "b").expect("rewrites");
        let h2 = mocks_tree_hash(&dir).expect("hashes");
        assert_ne!(h1, h2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pin_revision_parses() {
        let dir = std::env::temp_dir().join(format!("conf-pin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("creates");
        let pin = dir.join("pin.toml");
        std::fs::write(
            &pin,
            "# comment\nsdk_version = \"3.13.7\"\nsdk_revision = \"416e33b0\"\n",
        )
        .expect("writes");
        assert_eq!(sdk_revision(&pin).as_deref(), Ok("416e33b0"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn materialize_writes_root_tree_and_strips_markers() {
        let case = loaded(
            "diagnostics/x/t#0",
            &[(
                "lib/test.dart",
                "var x = 1;\n//        ^^^\n// [diag.undefinedIdentifier] y\n",
            )],
        );
        let mocks = std::env::temp_dir().join(format!("conf-mat-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&mocks);
        std::fs::create_dir_all(mocks.join("meta")).expect("creates mocks");
        let chunk = std::env::temp_dir().join(format!("conf-chunk-{}", std::process::id()));
        let errors = materialize(&[case], &[0], &chunk, &mocks);
        assert!(errors.is_empty(), "{errors:?}");
        let root = chunk.join("0");
        let test_dart =
            std::fs::read_to_string(root.join("lib/test.dart")).expect("reads test.dart");
        assert_eq!(test_dart, "var x = 1;\n", "markers stripped");
        assert_eq!(
            std::fs::read_to_string(root.join("pubspec.yaml")).expect("reads pubspec"),
            "name: test\n"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("analysis_options.yaml")).expect("reads options"),
            setup::default_options(None)
        );
        let config = std::fs::read_to_string(root.join(".dart_tool/package_config.json"))
            .expect("reads config");
        assert!(config.contains("\"languageVersion\":\"3.13\""), "{config}");
        assert!(config.contains("\"name\":\"test\""), "{config}");
        assert!(!config.contains("meta"), "{config}");
        let _ = std::fs::remove_dir_all(&chunk);
        let _ = std::fs::remove_dir_all(&mocks);
    }

    #[test]
    fn doc_range_markers_are_removed_for_doc_sourced_cases() {
        assert_eq!(remove_doc_range("a[!b!]c"), "abc");
        assert_eq!(remove_doc_range("no markers"), "no markers");
        assert_eq!(remove_doc_range("open [!only"), "open [!only");
        // Only the first pair goes, exactly like upstream.
        assert_eq!(remove_doc_range("[!a!] b [!c!]"), "a b [!c!]");
        // Legit index-negation code in a test-derived case is untouched.
        let case = Case {
            source: "src/diagnostics/x_test.dart#T".into(),
            ..Case::default()
        };
        assert_eq!(analysis_content(&case, "m[!flag];\n"), "m[!flag];\n");
        let doc = Case {
            source: "pkg/analyzer/messages.yaml#x".into(),
            ..Case::default()
        };
        assert_eq!(
            analysis_content(&doc, "abstract int [!f!] = 0;\n"),
            "abstract int f = 0;\n"
        );
    }

    #[test]
    fn aux_package_sections_materialize_and_register() {
        let mut case = loaded(
            "diagnostics/x/t#aux",
            &[
                (
                    "lib/test.dart",
                    "import 'package:aaa/a.dart';\nvar x = f;\n",
                ),
                ("package:aaa/lib/a.dart", "@deprecated\nlibrary a;\n"),
                ("package:p/p.dart", "var p = 1;\n"),
            ],
        );
        case.case.packages = vec!["meta".into(), "aaa".into(), "p".into()];
        case.case.aux_packages = vec!["p".into(), "p".into()]; // dup ignored
        let mocks = std::env::temp_dir().join(format!("conf-aux-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&mocks);
        std::fs::create_dir_all(mocks.join("meta/lib")).expect("creates mocks");
        let chunk = std::env::temp_dir().join(format!("conf-aux-chunk-{}", std::process::id()));
        let errors = materialize(&[case], &[0], &chunk, &mocks);
        assert!(errors.is_empty(), "{errors:?}");
        let root = chunk.join("0");
        // Inline aux files land under .aux/<name>/ in declared layout.
        assert_eq!(
            std::fs::read_to_string(root.join(".aux/aaa/lib/a.dart")).expect("reads aux file"),
            "@deprecated\nlibrary a;\n"
        );
        assert!(root.join(".aux/p/p.dart").is_file());
        let config = std::fs::read_to_string(root.join(".dart_tool/package_config.json"))
            .expect("reads config");
        // Inline aux packages registered with absolute file URIs and the
        // case language version; the meta mock is registered from the mocks
        // tree; the duplicate `p` header produced a single entry.
        assert!(config.contains("\"name\":\"aaa\""), "{config}");
        assert!(config.contains("\"rootUri\":\"file://"), "{config}");
        assert!(config.contains("\"packageUri\":\"lib/\""), "{config}");
        assert_eq!(config.matches("\"name\":\"p\"").count(), 1, "{config}");
        assert!(config.contains("\"name\":\"meta\""), "{config}");
        assert!(config.contains(".aux/aaa"), "{config}");
        // The meta entry points at the mocks tree.
        let meta_uri = format!(
            "\"rootUri\":\"{}\"",
            path_to_file_uri(&mocks.canonicalize().expect("canonical").join("meta"))
        );
        assert!(config.contains(&meta_uri), "{config}");
        let _ = std::fs::remove_dir_all(&chunk);
        let _ = std::fs::remove_dir_all(&mocks);
    }

    #[test]
    fn relative_chunk_dir_still_writes_absolute_aux_root_uris() {
        let mut case = loaded(
            "diagnostics/x/t#rel",
            &[
                (
                    "lib/test.dart",
                    "import 'package:aaa/a.dart';\nvar x = f;\n",
                ),
                ("package:aaa/lib/a.dart", "var a = 1;\n"),
            ],
        );
        case.case.packages = vec!["aaa".into()];
        let mocks = std::env::temp_dir().join(format!("conf-rel-mocks-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&mocks);
        std::fs::create_dir_all(&mocks).expect("creates mocks");
        // A relative chunk dir, the shape the runner passes for the tracked
        // `target/conformance/chunks` work root: the aux rootUri must still
        // come out absolute (and symlink-resolved), or the analyzer cannot
        // resolve the package.
        let rel = Path::new("target").join(format!("conf-rel-chunk-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&rel);
        let errors = materialize(&[case], &[0], &rel, &mocks);
        assert!(errors.is_empty(), "{errors:?}");
        let config = std::fs::read_to_string(rel.join("0/.dart_tool/package_config.json"))
            .expect("reads config");
        let aux_root = std::fs::canonicalize(&rel)
            .expect("canonical chunk")
            .join("0/.aux/aaa");
        assert!(
            config.contains(&format!("\"rootUri\":\"{}\"", path_to_file_uri(&aux_root))),
            "{config}"
        );
        let _ = std::fs::remove_dir_all(&rel);
        let _ = std::fs::remove_dir_all(&mocks);
    }

    #[test]
    fn file_uri_encoding() {
        assert_eq!(
            path_to_file_uri(Path::new("/home/analyst/x y")),
            "file:///home/analyst/x%20y"
        );
        assert_eq!(
            path_to_file_uri(Path::new("/a.b-c_d~e")),
            "file:///a.b-c_d~e"
        );
    }

    #[test]
    fn materialize_reports_missing_mock() {
        let mut case = loaded("diagnostics/x/t#1", &[("lib/test.dart", "var x = 1;\n")]);
        case.case.packages = vec!["no_such_mock".into()];
        let mocks = std::env::temp_dir().join(format!("conf-mat2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&mocks);
        std::fs::create_dir_all(&mocks).expect("creates");
        let chunk = std::env::temp_dir().join(format!("conf-chunk2-{}", std::process::id()));
        let errors = materialize(&[case], &[0], &chunk, &mocks);
        assert_eq!(errors.len(), 1);
        assert!(errors[&0].contains("no_such_mock"));
        let _ = std::fs::remove_dir_all(&chunk);
        let _ = std::fs::remove_dir_all(&mocks);
    }

    /// A temp corpus dir holding one case per (id, files) entry.
    fn temp_corpus(label: &str, cases: &[(&str, &[(&str, &str)])]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("conf-corpus-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("creates corpus dir");
        for (id, files) in cases {
            let text = case::write(&loaded(id, files).case);
            let name = format!("{}.case", id.replace('/', "_"));
            std::fs::write(dir.join(name), text).expect("writes case");
        }
        dir
    }

    #[test]
    fn load_corpus_merges_manual_deduped_extracted_wins() {
        let extracted = temp_corpus(
            "ext",
            &[("diagnostics/a/1", &[("lib/test.dart", "extracted\n")])],
        );
        let manual = temp_corpus(
            "man",
            &[
                ("manual/diagnostic/b/1", &[("lib/test.dart", "manual b\n")]),
                ("diagnostics/a/1", &[("lib/test.dart", "manual collides\n")]),
            ],
        );
        let cases = load_corpus(&extracted, Some(&manual)).expect("loads");
        let ids: Vec<&str> = cases.iter().map(|c| c.case.id.as_str()).collect();
        assert_eq!(ids, vec!["diagnostics/a/1", "manual/diagnostic/b/1"]);
        assert_eq!(cases[0].case.files[0].1, "extracted\n", "extracted wins");
        assert_eq!(cases[1].case.files[0].1, "manual b\n");
        // Without the manual corpus only the extracted case loads; a missing
        // manual directory is reported and skipped, not fatal.
        assert_eq!(load_corpus(&extracted, None).expect("loads").len(), 1);
        let missing = extracted.parent().expect("tmp").join("no-such-manual");
        assert_eq!(
            load_corpus(&extracted, Some(&missing))
                .expect("loads")
                .len(),
            1
        );
        let _ = std::fs::remove_dir_all(&extracted);
        let _ = std::fs::remove_dir_all(&manual);
    }

    #[test]
    fn supplied_pubspec_wins_generated_otherwise_and_never_blocks() {
        let supplied = loaded(
            "manual/diagnostic/missing_name/1",
            &[
                ("lib/test.dart", "void f() {}\n"),
                ("pubspec.yaml", "version: 1.0.0\n"),
            ],
        );
        let generated = loaded("diagnostics/x/t", &[("lib/test.dart", "var x = 1;\n")]);
        let invalid = loaded(
            "manual/diagnostic/invalid_pubspec/1",
            &[
                ("lib/test.dart", "void f() {}\n"),
                ("pubspec.yaml", "- a\n- b\n"),
            ],
        );
        let cases = vec![supplied, generated, invalid];
        let mocks = std::env::temp_dir().join(format!("conf-pubspec-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&mocks);
        std::fs::create_dir_all(&mocks).expect("creates mocks");
        let chunk = std::env::temp_dir().join(format!("conf-pubspec-chunk-{}", std::process::id()));
        let errors = materialize(&cases, &[0, 1, 2], &chunk, &mocks);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(
            std::fs::read_to_string(chunk.join("0/pubspec.yaml")).expect("reads"),
            "version: 1.0.0\n",
            "supplied pubspec used verbatim"
        );
        assert_eq!(
            std::fs::read_to_string(chunk.join("1/pubspec.yaml")).expect("reads"),
            "name: test\n",
            "generated pubspec unchanged"
        );
        // A syntactically valid but non-map pubspec still materializes: the
        // oracle is supposed to report the pubspec error.
        assert_eq!(
            std::fs::read_to_string(chunk.join("2/pubspec.yaml")).expect("reads"),
            "- a\n- b\n"
        );
        let _ = std::fs::remove_dir_all(&chunk);
        let _ = std::fs::remove_dir_all(&mocks);
    }

    #[test]
    fn analysis_options_section_honored_directive_wins() {
        let section_only = loaded(
            "manual/diagnostic/x/1",
            &[
                ("lib/test.dart", "void f() {}\n"),
                (
                    "analysis_options.yaml",
                    "linter:\n  rules:\n    - avoid_print\n",
                ),
            ],
        );
        let mut both = loaded(
            "manual/diagnostic/x/2",
            &[
                ("lib/test.dart", "void f() {}\n"),
                (
                    "analysis_options.yaml",
                    "linter:\n  rules:\n    - avoid_print\n",
                ),
            ],
        );
        both.case.options = Some("analyzer:\n  errors:\n    dead_code: ignore\n".into());
        let cases = vec![section_only, both];
        let mocks = std::env::temp_dir().join(format!("conf-options-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&mocks);
        std::fs::create_dir_all(&mocks).expect("creates mocks");
        let chunk = std::env::temp_dir().join(format!("conf-options-chunk-{}", std::process::id()));
        let errors = materialize(&cases, &[0, 1], &chunk, &mocks);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(
            std::fs::read_to_string(chunk.join("0/analysis_options.yaml")).expect("reads"),
            "linter:\n  rules:\n    - avoid_print\n",
            "a section without a directive is honored verbatim"
        );
        assert_eq!(
            std::fs::read_to_string(chunk.join("1/analysis_options.yaml")).expect("reads"),
            "analyzer:\n  errors:\n    dead_code: ignore\n",
            "the options directive wins over a section"
        );
        let _ = std::fs::remove_dir_all(&chunk);
        let _ = std::fs::remove_dir_all(&mocks);
    }

    #[test]
    fn manual_corpus_is_canonical() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../conformance/manual");
        assert!(dir.is_dir(), "{} should exist", dir.display());
        let mut files = Vec::new();
        collect_case_files(&dir, &mut files);
        assert!(
            files.len() > 100,
            "manual corpus unexpectedly small: {files:?}"
        );
        let mut checked = 0usize;
        for path in files {
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            let parsed =
                case::read(&text).unwrap_or_else(|e| panic!("{}: {:?}", path.display(), e));
            assert_eq!(
                case::write(&parsed),
                text,
                "{} is not in canonical form",
                path.display()
            );
            checked += 1;
        }
        assert!(checked > 100);
    }
}
