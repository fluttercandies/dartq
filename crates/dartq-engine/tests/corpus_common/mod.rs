//! Shared harness for the corpus-slice integration tests: one data-driven
//! test function per diagnostic family reads its `.case` slice at runtime,
//! materializes each case into a throwaway package root, runs the engine
//! over it, and compares the canonical diagnostic rows against
//! `conformance/expected.tsv` — the same comparison the chunked conformance
//! runner performs, scoped to a single family so a fix lane can self-verify
//! with `cargo test --test corpus_<family> -- --exact`.
//!
//! The case format, marker stripping, materialization, row model and
//! expected-cell vocabulary are ported verbatim from the conformance
//! harness (`tools/conformance/src/{case,markers,materialize,rows}.rs`) so
//! the slice tests compare byte-identically to the census sweep.
//!
//! # Environment
//!
//! The tests need the pinned Dart SDK on disk (cases import `dart:`
//! libraries). Resolution order: `DART_SDK` env, `$HOME/.dartq-oracle/*`
//! (the oracle pin layout), then a `dart` executable on `PATH`. The case
//! corpus itself is generated: `target/conformance/cases/` must exist
//! (`conformance extract` produces it); a missing corpus fails the test
//! with instructions rather than passing vacuously.

#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, OnceLock};

use dartq_engine::{Engine, EngineConfig, memory_store};
use dartq_package::PackageTable;
use dartq_pub::{LockFile, PackageConfig, Pubspec, build_graph};
use dartq_sdk::SdkSnapshot;

// ---------------------------------------------------------------------------
// Repository layout
// ---------------------------------------------------------------------------

/// Repository root: `crates/dartq-engine` → two levels up.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repo root resolves")
}

fn cases_dir() -> PathBuf {
    repo_root().join("target/conformance/cases")
}

fn manual_dir() -> PathBuf {
    repo_root().join("conformance/manual")
}

fn mocks_root() -> PathBuf {
    repo_root().join("conformance/mocks")
}

fn expected_tsv() -> PathBuf {
    repo_root().join("conformance/expected.tsv")
}

// ---------------------------------------------------------------------------
// `.case` format — port of tools/conformance/src/case.rs
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct Case {
    pub id: String,
    pub source: String,
    pub language: Option<String>,
    pub packages: Vec<String>,
    pub aux_packages: Vec<String>,
    pub options: Option<String>,
    pub ignores: Vec<String>,
    pub files: Vec<(String, String)>,
}

fn join_continuations(lines: &[String]) -> String {
    let mut joined = lines.join("\n");
    if !joined.is_empty() {
        joined.push('\n');
    }
    joined
}

/// Parses canonical case bytes (port of `case::read`).
pub fn read_case(text: &str) -> Result<Case, String> {
    let mut case = Case::default();
    let mut current: Option<(String, Vec<String>)> = None;
    let mut section = Section::Header;
    let mut options: Vec<String> = Vec::new();

    macro_rules! flush_file {
        () => {
            if let Some((path, lines)) = current.take() {
                case.files.push((path, join_continuations(&lines)));
            }
        };
    }
    macro_rules! flush_options {
        () => {
            if section == Section::Options {
                case.options = Some(options.join("\n"));
                options.clear();
            }
        };
    }

    for (no, raw) in text.lines().enumerate() {
        if let Some(rest) = raw.strip_prefix("//- ") {
            flush_file!();
            flush_options!();
            section = Section::Header;
            let (key, value) = rest.split_once(": ").unwrap_or((rest, ""));
            let key = key.strip_suffix(':').unwrap_or(key);
            match key {
                "id" if value.is_empty() => return Err(format!("line {no}: empty id")),
                "id" => case.id = value.to_string(),
                "source" => case.source = value.to_string(),
                "language" => case.language = Some(value.to_string()),
                "packages" => {
                    case.packages = value
                        .split(", ")
                        .filter(|p| !p.is_empty())
                        .map(str::to_string)
                        .collect();
                }
                "package" => case.aux_packages.push(value.to_string()),
                "ignore" => {
                    case.ignores = value
                        .split(", ")
                        .filter(|p| !p.is_empty())
                        .map(str::to_string)
                        .collect();
                }
                "options" if value.is_empty() => section = Section::Options,
                "options" => {
                    return Err(format!("line {no}: options directive carries inline text"));
                }
                "file" => {
                    if value.is_empty() {
                        return Err(format!("line {no}: empty file path"));
                    }
                    current = Some((value.to_string(), Vec::new()));
                    section = Section::File;
                }
                other => return Err(format!("line {no}: unknown directive `{other}`")),
            }
        } else if raw == "//-" {
            return Err(format!("line {no}: bare directive line"));
        } else if raw == "//+" {
            match section {
                Section::Options => options.push(String::new()),
                Section::File => current
                    .as_mut()
                    .ok_or_else(|| format!("line {no}: continuation outside a section"))?
                    .1
                    .push(String::new()),
                Section::Header => {
                    return Err(format!("line {no}: continuation outside a section"));
                }
            }
        } else if let Some(rest) = raw.strip_prefix("//+ ") {
            match section {
                Section::Options => options.push(rest.to_string()),
                Section::File => current
                    .as_mut()
                    .ok_or_else(|| format!("line {no}: continuation outside a section"))?
                    .1
                    .push(rest.to_string()),
                Section::Header => {
                    return Err(format!("line {no}: continuation outside a section"));
                }
            }
        } else {
            return Err(format!("line {no}: non-directive line `{raw}`"));
        }
    }
    flush_options!();
    flush_file!();
    if case.id.is_empty() {
        return Err("missing id directive".to_string());
    }
    if case.files.is_empty() {
        return Err("missing file section".to_string());
    }
    Ok(case)
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum Section {
    Header,
    Options,
    File,
}

// ---------------------------------------------------------------------------
// Markers — port of tools/conformance/src/markers.rs (removal half only)
// ---------------------------------------------------------------------------

fn is_caret_line(text: &str) -> bool {
    let rest = text.trim_start_matches([' ', '\t']);
    let Some(rest) = rest.strip_prefix("//") else {
        return false;
    };
    let rest = rest.trim_start_matches([' ', '\t']);
    let Some(caret_end) = rest.find(|c| c != '^') else {
        return !rest.is_empty();
    };
    rest[caret_end..].trim().is_empty()
}

fn is_expectation_line(text: &str) -> bool {
    let rest = text.trim_start_matches([' ', '\t']);
    let Some(rest) = rest.strip_prefix("//") else {
        return false;
    };
    let rest = rest.trim_start_matches([' ', '\t']);
    let Some(rest) = rest.strip_prefix('[') else {
        return false;
    };
    if let Some(rest) = rest.strip_prefix("diag.") {
        let mut chars = rest.chars();
        if !chars
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        {
            return false;
        }
        let name_len = rest[1..]
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .map_or(rest.len() - 1, |e| e + 1);
        return rest[name_len..].starts_with(']');
    }
    if let Some(rest) = rest.strip_prefix("context") {
        let digits = rest.trim_start_matches([' ', '\t']);
        let num_len = digits
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(digits.len());
        if num_len == 0 {
            return false;
        }
        let after = &digits[num_len..];
        return after.starts_with(']');
    }
    false
}

struct Line<'a> {
    text: &'a str,
    terminator: &'a str,
}

fn parse_lines(content: &str) -> Vec<Line<'_>> {
    let mut lines = Vec::new();
    let bytes = content.as_bytes();
    let mut start = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'\r' | b'\n' => {
                let mut end = i + 1;
                if bytes[i] == b'\r' && bytes.get(i + 1) == Some(&b'\n') {
                    end += 1;
                }
                lines.push(Line {
                    text: &content[start..i],
                    terminator: &content[i..end],
                });
                start = end;
                i = end;
            }
            _ => i += 1,
        }
    }
    lines.push(Line {
        text: &content[start..],
        terminator: "",
    });
    lines
}

fn is_marker_line(text: &str) -> bool {
    is_caret_line(text) || is_expectation_line(text)
}

/// Removes diagnostic expectation marker lines (port of
/// `markers::remove_expectations`).
fn remove_expectations(content: &str) -> String {
    let lines = parse_lines(content);
    let kept: Vec<&Line<'_>> = lines.iter().filter(|l| !is_marker_line(l.text)).collect();
    let mut out = String::with_capacity(content.len());
    for (i, line) in kept.iter().enumerate() {
        out.push_str(line.text);
        if i + 1 < kept.len() {
            out.push_str(line.terminator);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Materialization — port of tools/conformance/src/materialize.rs
// ---------------------------------------------------------------------------

fn is_doc_sourced(case: &Case) -> bool {
    case.source.contains("messages.yaml#")
}

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

fn analysis_content(case: &Case, content: &str) -> String {
    let stripped = remove_expectations(content);
    if is_doc_sourced(case) {
        remove_doc_range(&stripped)
    } else {
        stripped
    }
}

/// The default `analysis_options.yaml` when a case carries none: the
/// official pub-package test defaults (port of `setup::default_options`).
fn default_options() -> String {
    "analyzer:\n  enable-experiment:\n    - augmentations\n    - enhanced-parts\n    - static-extensions\n    - this-promotion\n    - variance\n  optional-checks:\n    propagate-linter-exceptions: true\n".to_string()
}

fn effective_options(options: Option<&str>) -> String {
    match options {
        Some(yaml) => yaml.to_string(),
        None => default_options(),
    }
}

fn language_version(case: &Case) -> &str {
    case.language.as_deref().unwrap_or("3.13")
}

/// Section paths are relative to the package root; `..` rejected (port of
/// `materialize::normalize_section_path`).
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

/// `file://` URI of an absolute path (port of `materialize::path_to_file_uri`).
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

/// Writes one case's package root: file sections (plain + `package:` inline
/// aux), generated-or-supplied `pubspec.yaml`, effective
/// `analysis_options.yaml`, and `.dart_tool/package_config.json` registering
/// `test`, inline aux packages, and mock-backed names (port of
/// `materialize::materialize_one`).
///
/// `root` must already exist and be canonicalized. Returns `Err` with the
/// materialization failure (missing mock, escaping path) — the caller
/// records it as a case-level failure, never silently skips.
fn materialize_case(case: &Case, root: &Path, mocks_abs: &Path) -> Result<(), String> {
    let mut sections: BTreeMap<String, String> = BTreeMap::new();
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
    let lang = language_version(case).to_string();
    let mut config_packages = vec![serde_json::json!({
        "name": "test",
        "rootUri": "../",
        "packageUri": "lib/",
        "languageVersion": lang,
    })];
    for name in inline_aux.keys() {
        let aux_root = root.join(".aux").join(name);
        config_packages.push(serde_json::json!({
            "name": name,
            "rootUri": path_to_file_uri(&aux_root),
            "packageUri": "lib/",
            "languageVersion": lang,
        }));
    }
    let mut mock_names: BTreeSet<&str> = BTreeSet::new();
    for name in case.packages.iter().chain(case.aux_packages.iter()) {
        if inline_aux.contains_key(name) {
            continue;
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

// ---------------------------------------------------------------------------
// Rows — port of tools/conformance/src/rows.rs
// ---------------------------------------------------------------------------

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Row {
    pub severity: String,
    pub code: String,
    pub file: String,
    pub line: u32,
    pub column: u32,
    pub length: u64,
    pub message: String,
}

fn sanitize(message: &str) -> String {
    let mut out = String::with_capacity(message.len());
    for c in message.chars() {
        match c {
            '\n' | '\r' | '\t' => out.push(' '),
            _ => out.push(c),
        }
    }
    out
}

impl Row {
    #[allow(clippy::too_many_arguments)]
    fn new(
        severity: String,
        code: String,
        file: String,
        line: u32,
        column: u32,
        length: u64,
        message: String,
    ) -> Row {
        Row {
            severity,
            code,
            file,
            line,
            column,
            length,
            message: sanitize(&message),
        }
    }

    fn text(&self) -> String {
        format!(
            "{}|{}|{}|{}:{}|{}|{}",
            self.severity,
            self.code,
            self.file,
            self.line,
            self.column,
            self.length,
            sanitize(&self.message)
        )
    }

    fn parse_text(text: &str) -> Option<Row> {
        let parts: Vec<&str> = text.splitn(6, '|').collect();
        if parts.len() != 6 {
            return None;
        }
        let (line, column) = parts[3].split_once(':')?;
        Some(Row::new(
            parts[0].to_string(),
            parts[1].to_string(),
            parts[2].to_string(),
            line.parse().ok()?,
            column.parse().ok()?,
            parts[4].parse().ok()?,
            parts[5].to_string(),
        ))
    }

    fn cmp_text(&self, other: &Row) -> std::cmp::Ordering {
        self.severity
            .cmp(&other.severity)
            .then(self.code.cmp(&other.code))
            .then(self.file.cmp(&other.file))
            .then(self.line.cmp(&other.line))
            .then(self.column.cmp(&other.column))
            .then(self.length.cmp(&other.length))
            .then(self.message.cmp(&other.message))
    }
}

fn sort_rows(rows: &mut Vec<Row>) {
    rows.sort_by(|a, b| a.cmp_text(b));
}

fn diff_sorted<'a>(expected: &'a [Row], actual: &'a [Row]) -> (Vec<&'a Row>, Vec<&'a Row>) {
    let mut missing = Vec::new();
    let mut extra = Vec::new();
    let mut i = 0usize;
    let mut j = 0usize;
    while i < expected.len() && j < actual.len() {
        match expected[i].cmp_text(&actual[j]) {
            std::cmp::Ordering::Less => {
                missing.push(&expected[i]);
                i += 1;
            }
            std::cmp::Ordering::Greater => {
                extra.push(&actual[j]);
                j += 1;
            }
            std::cmp::Ordering::Equal => {
                i += 1;
                j += 1;
            }
        }
    }
    missing.extend(expected[i..].iter());
    extra.extend(actual[j..].iter());
    (missing, extra)
}

#[derive(Debug, PartialEq, Eq, Clone)]
enum Expected {
    Rows(Vec<Row>),
    Crash,
}

impl Expected {
    fn parse(cell: &str) -> Option<Expected> {
        match cell {
            "-" => Some(Expected::Rows(Vec::new())),
            "oracle_crash" => Some(Expected::Crash),
            _ => {
                let mut rows = Vec::new();
                for text in cell.split('\t') {
                    rows.push(Row::parse_text(text)?);
                }
                Some(Expected::Rows(rows))
            }
        }
    }

    fn rows(&self) -> &[Row] {
        match self {
            Expected::Rows(rows) => rows,
            Expected::Crash => &[],
        }
    }
}

// ---------------------------------------------------------------------------
// expected.tsv — parsed once, filtered per family
// ---------------------------------------------------------------------------

fn expected_map() -> &'static BTreeMap<String, Expected> {
    static MAP: OnceLock<BTreeMap<String, Expected>> = OnceLock::new();
    MAP.get_or_init(|| {
        let text = std::fs::read_to_string(expected_tsv())
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", expected_tsv().display()));
        text.lines()
            .filter(|line| !line.is_empty())
            .filter_map(|line| {
                let (id, cell) = line.split_once('\t')?;
                Expected::parse(cell).map(|e| (id.to_string(), e))
            })
            .collect()
    })
}

// ---------------------------------------------------------------------------
// Case discovery — walk the family's case directory
// ---------------------------------------------------------------------------

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

/// The case directory a prefix slices: the family root, not the prefix
/// itself — a sub-family prefix like `diagnostics/<fam>/<TestClass>` still
/// walks the whole family dir and filters by id, so flat `doc_`/`fix_`
/// siblings stay reachable from a class-level slice.
fn family_dir(prefix: &str) -> PathBuf {
    let parts: Vec<&str> = prefix.split('/').collect();
    // The family root is the first two segments for `diagnostics/<fam>`;
    // for `manual/diagnostic/<fam>` the `manual/` infix is already inside
    // `manual_dir()`, so the on-disk root is `diagnostic/<fam>`.
    let (base, head): (PathBuf, Vec<&str>) = if prefix.starts_with("manual/") {
        (
            manual_dir(),
            parts.iter().skip(1).take(2).copied().collect(),
        )
    } else {
        (cases_dir(), parts.iter().take(2).copied().collect())
    };
    base.join(head.join("/"))
}

/// Loads every `.case` under the family's directory whose `id:` directive
/// starts with `prefix`, sorted by id (port of `materialize::load_cases`
/// scoped to one family).
fn load_family_cases(prefix: &str) -> Result<Vec<(PathBuf, Case)>, String> {
    let dir = family_dir(prefix);
    if !dir.is_dir() {
        return Err(format!(
            "case corpus {} is missing; run `conformance extract` first",
            dir.display()
        ));
    }
    let mut files = Vec::new();
    collect_case_files(&dir, &mut files);
    let mut cases: Vec<(PathBuf, Case)> = Vec::with_capacity(files.len());
    let mut by_id: BTreeMap<String, PathBuf> = BTreeMap::new();
    for path in files {
        let bytes =
            std::fs::read(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        let text =
            String::from_utf8(bytes).map_err(|_| format!("{} is not UTF-8", path.display()))?;
        let parsed = read_case(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        if !parsed.id.starts_with(prefix) {
            continue;
        }
        if let Some(first) = by_id.insert(parsed.id.clone(), path.clone()) {
            return Err(format!(
                "duplicate case id `{}` in {} and {}",
                parsed.id,
                first.display(),
                path.display()
            ));
        }
        cases.push((path, parsed));
    }
    cases.sort_by(|a, b| a.1.id.cmp(&b.1.id));
    Ok(cases)
}

// ---------------------------------------------------------------------------
// SDK resolution — runtime, no literal paths in tracked files
// ---------------------------------------------------------------------------

/// Resolves the Dart SDK root: `DART_SDK` env → `$HOME/.dartq-oracle/*`
/// (pinned oracle layout) → `dart` on `PATH` (ancestor of the binary
/// carrying `lib/libraries.json`).
fn sdk_root() -> PathBuf {
    if let Some(dir) = std::env::var_os("DART_SDK") {
        let dir = PathBuf::from(dir);
        if looks_like_sdk(&dir) {
            return dir;
        }
        panic!("DART_SDK={} is not a Dart SDK root", dir.display());
    }
    if let Some(home) = std::env::var_os("HOME") {
        let oracle = PathBuf::from(home).join(".dartq-oracle");
        if let Ok(entries) = std::fs::read_dir(&oracle) {
            let mut candidates: Vec<PathBuf> = entries
                .flatten()
                .map(|e| e.path().join("dart-sdk"))
                .filter(|p| looks_like_sdk(p))
                .collect();
            candidates.sort();
            if let Some(first) = candidates.into_iter().next() {
                return first;
            }
        }
    }
    // `dart` on PATH: <sdk>/bin/dart → sdk root is two levels up.
    if let Some(path) = find_on_path("dart") {
        if let Some(bin) = path.parent() {
            if let Some(root) = bin.parent() {
                if looks_like_sdk(root) {
                    return root.to_path_buf();
                }
            }
        }
    }
    panic!(
        "no Dart SDK found: set DART_SDK, install under ~/.dartq-oracle/, or put `dart` on PATH"
    );
}

fn looks_like_sdk(dir: &Path) -> bool {
    dir.join("version").is_file()
        && dir.join("lib/libraries.json").is_file()
        && dir.join("lib/_internal/allowed_experiments.json").is_file()
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&paths) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Engine assembly + run — one case → canonical rows
// ---------------------------------------------------------------------------

static TEMP_COUNTER: AtomicU32 = AtomicU32::new(0);

/// The engine's diagnostic output as canonical rows, attributed to the
/// case root exactly like the runner's `attribute` (file relative to the
/// case root, message root replaced by `<chunk>`).
struct CaseRun {
    rows: Vec<Row>,
    /// Diagnostics whose file fell outside the case root (dropped, counted —
    /// the runner's `outside` counter; reported so nothing drops silently).
    outside: usize,
    /// Materialization or engine-construction failure text.
    error: Option<String>,
}

fn run_case(case: &Case, case_root: &Path, sdk: &Path, mocks_abs: &Path) -> CaseRun {
    let mut run = CaseRun {
        rows: Vec::new(),
        outside: 0,
        error: None,
    };
    if let Err(e) = materialize_case(case, case_root, mocks_abs) {
        run.error = Some(format!("materialize: {e}"));
        return run;
    }
    // Package assembly — the same faces the CLI's `package_assembly_with`
    // consumes: parse the written config, empty lock (corpus cases carry
    // no pubspec.lock), pubspec from disk (empty model on parse failure),
    // graph + identity table.
    let config_dir = case_root.join(".dart_tool");
    let config_text = match std::fs::read_to_string(config_dir.join("package_config.json")) {
        Ok(text) => text,
        Err(e) => {
            run.error = Some(format!("read package_config.json: {e}"));
            return run;
        }
    };
    let config = match PackageConfig::parse(&config_text) {
        Ok(config) => Arc::new(config),
        Err(e) => {
            run.error = Some(format!("parse package_config.json: {e}"));
            return run;
        }
    };
    let lock = LockFile::empty();
    let pubspec_text = std::fs::read_to_string(case_root.join("pubspec.yaml")).unwrap_or_default();
    let pubspec = Pubspec::parse_with_overrides(&pubspec_text, None).unwrap_or_else(|_| Pubspec {
        name: String::new(),
        version: None,
        environment: Default::default(),
        workspace: Vec::new(),
        resolution: Default::default(),
        dependencies: Default::default(),
        dev_dependencies: Default::default(),
        dependency_overrides: Default::default(),
    });
    let graph = match build_graph(
        &config,
        &lock,
        &pubspec,
        &BTreeMap::new(),
        None,
        &config_dir,
    ) {
        Ok(graph) => graph,
        Err(e) => {
            run.error = Some(format!("build_graph: {e}"));
            return run;
        }
    };
    let packages = match PackageTable::build(&config, &graph, &config_dir) {
        Ok(packages) => packages,
        Err(e) => {
            run.error = Some(format!("PackageTable::build: {e}"));
            return run;
        }
    };
    let sdk_snapshot = match SdkSnapshot::load(sdk, None) {
        Ok(snapshot) => snapshot,
        Err(e) => {
            run.error = Some(format!("SdkSnapshot::load: {e}"));
            return run;
        }
    };
    let mut engine = match Engine::new(EngineConfig {
        store: memory_store(),
        sdk: sdk_snapshot,
        packages,
        pub_config: Arc::clone(&config),
        config_dir: config_dir.clone(),
        context_roots: Vec::new(),
        semdiag: Some(dartq_engine::semdiag::diagnoser()),
        target: dartq_engine::DEFAULT_TARGET.to_owned(),
        declared_variables: rustc_hash::FxHashMap::default(),
        experiments: Vec::new(),
    }) {
        Ok(engine) => engine,
        Err(e) => {
            run.error = Some(format!("Engine::new: {e}"));
            return run;
        }
    };
    if let Err(e) = engine.discover_workspace() {
        run.error = Some(format!("discover_workspace: {e}"));
        return run;
    }
    let diagnostics = match engine.analyze(
        &[],
        &mut |_| {},
        &dartq_engine::CancellationToken::default(),
    ) {
        Ok(diagnostics) => diagnostics,
        Err(_) => {
            run.error = Some("analyze cancelled".to_string());
            return run;
        }
    };
    let root_str = case_root.to_string_lossy().into_owned();
    for diag in &diagnostics {
        let Some(path) = engine.file_table().path(diag.file) else {
            run.outside += 1;
            continue;
        };
        let rel = match path.strip_prefix(case_root) {
            Ok(rel) => rel,
            Err(_) => {
                run.outside += 1;
                continue;
            }
        };
        let file = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        if file.is_empty() {
            run.outside += 1;
            continue;
        }
        let (line, column) = match (engine.line_index(diag.file), engine.content(diag.file)) {
            (Some(index), Some(bytes)) => {
                let text = std::str::from_utf8(bytes).unwrap_or("");
                index.line_col_utf16(text, diag.range.offset)
            }
            _ => {
                run.outside += 1;
                continue;
            }
        };
        let message = diag.message.as_str().replace(root_str.as_str(), "<chunk>");
        run.rows.push(Row::new(
            diag.severity.as_machine_str().to_string(),
            diag.code.name.to_string(),
            file,
            line,
            column,
            u64::from(diag.range.length),
            message,
        ));
    }
    sort_rows(&mut run.rows);
    run
}

// ---------------------------------------------------------------------------
// Family runner — the one test fn's body
// ---------------------------------------------------------------------------

/// Runs every case of the family slice `prefix` (an expected.tsv id prefix
/// like `diagnostics/cast_to_non_type`) and asserts zero divergences.
///
/// Every case is accounted: pass / fail (with per-row missing+extra detail)
/// / oracle_crash (skipped, counted) / materialize-or-engine error (failed
/// with the error text). The assertion collects ALL divergences before
/// failing — never panic-loops on the first.
pub fn run_family(prefix: &str) {
    // The family's case root: the extracted corpus for `diagnostics/*`
    // families, the tracked `conformance/manual` tree for `manual/*`.
    // CI's rust job cannot run `conformance extract` — the extract
    // source is the pinned upstream SDK tree, absent on the runner —
    // so a wholly absent root self-skips loudly, the same posture as
    // ci.yml's dart-gated plugin fixtures. A root that exists while
    // this family's slice is missing stays a hard panic below: that is
    // real extraction drift, not a missing corpus.
    let root = if prefix.starts_with("manual/") {
        manual_dir()
    } else {
        cases_dir()
    };
    if !root.is_dir() {
        eprintln!(
            "corpus root {} absent; skipping family {prefix} (run `conformance extract` to enable)",
            root.display()
        );
        return;
    }
    let cases = load_family_cases(prefix).unwrap_or_else(|e| panic!("family {prefix}: {e}"));
    assert!(!cases.is_empty(), "family {prefix}: no cases found");
    let expected = expected_map();
    let sdk = sdk_root();
    let mocks = mocks_root();
    let mocks_abs = std::fs::canonicalize(&mocks).unwrap_or(mocks);

    let mut pass = 0usize;
    let mut oracle_crash = 0usize;
    let mut failures: Vec<String> = Vec::new();

    for (_, case) in &cases {
        let id = &case.id;
        let exp = match expected.get(id) {
            Some(Expected::Crash) => {
                oracle_crash += 1;
                continue;
            }
            Some(Expected::Rows(rows)) => rows,
            None => {
                failures.push(format!("{id}: no expected.tsv entry"));
                continue;
            }
        };
        // Per-case throwaway package root.
        let tag = format!(
            "corpus-{}-{}-{}",
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed),
            id.replace('/', "_").replace('#', "_")
        );
        let root = std::env::temp_dir().join(tag);
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create case root");
        let root = root.canonicalize().expect("canonical case root");

        let outcome = run_case(case, &root, &sdk, &mocks_abs);
        let _ = std::fs::remove_dir_all(&root);

        if let Some(error) = outcome.error {
            failures.push(format!("{id}: {error}"));
            continue;
        }
        let (missing, extra) = diff_sorted(exp, &outcome.rows);
        if missing.is_empty() && extra.is_empty() {
            pass += 1;
        } else {
            let mut detail = format!("{id}:");
            for row in &missing {
                detail.push_str(&format!("\n  missing: {}", row.text()));
            }
            for row in &extra {
                detail.push_str(&format!("\n  extra:   {}", row.text()));
            }
            if outcome.outside > 0 {
                detail.push_str(&format!(
                    "\n  outside-root rows dropped: {}",
                    outcome.outside
                ));
            }
            failures.push(detail);
        }
    }

    eprintln!(
        "corpus family {prefix}: {} cases — {pass} pass, {} oracle_crash, {} failed",
        cases.len(),
        oracle_crash,
        failures.len()
    );
    assert!(
        failures.is_empty(),
        "{} of {} cases diverged:\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n")
    );
}
