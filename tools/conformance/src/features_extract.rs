//! Extraction of language-feature cases from the two static-error suites of
//! the pinned SDK: `tests/language/**` and the co19 suite's
//! `LanguageFeatures/**` (same comment-marker format). Every test file
//! becomes at least one case:
//!
//! - multitest files (`//# key: outcome` lines) expand one case per section
//!   plus the `none` section, exactly like the official generator: each
//!   section keeps only its own tagged lines (other sections get a blank
//!   line) and all untagged lines; sections without a valid outcome are
//!   dropped the same way;
//! - every other test file is one case, analyzed with the file unchanged
//!   (the `// [analyzer] ...` expectation markers are plain comments and are
//!   part of the analyzed text, as in the official static-error runner).
//!
//! Files imported or parted by a relative path travel with the case as extra
//! file sections (transitively, every path resolved against the main test's
//! directory, mirroring the official relative-import collector).
//!
//! `// @dart = X.Y` headers become the case language version and
//! `// SharedOptions=--enable-experiment=...` headers become
//! `enable-experiment` options. Each case is attributed to a feature flag by
//! its suite directory (see `LANGUAGE_FLAG_BY_DIR` / `CO19_FLAG_BY_DIR`);
//! directories with no obvious flag land under `_core`.

use crate::case::Case;
use crate::setup;
use std::collections::BTreeSet;
use std::path::Path;

/// Tests/language top-level directory -> feature flag (the keys of the
/// vendored experimental features table); unlisted directories are core
/// language behavior (`_core/<dir>`).
pub const LANGUAGE_FLAG_BY_DIR: &[(&str, &str)] = &[
    ("anonymous_methods", "anonymous-methods"),
    ("class_modifiers", "class-modifiers"),
    ("constants_2018", "constant-update-2018"),
    ("const_functions", "const-functions"),
    ("control_flow_collections", "control-flow-collections"),
    ("dot_shorthands", "dot-shorthands"),
    ("extension_methods", "extension-methods"),
    ("extension_type", "inline-class"),
    ("inference_update_1", "inference-update-1"),
    ("inference_update_2", "inference-update-2"),
    ("inference_update_3", "inference-update-3"),
    ("inference_update_4", "inference-update-4"),
    ("inference_using_bounds", "inference-using-bounds"),
    ("named_arguments_anywhere", "named-arguments-anywhere"),
    ("nonfunction_type_aliases", "nonfunction-type-aliases"),
    ("nnbd", "non-nullable"),
    ("null_aware_elements", "null-aware-elements"),
    ("patterns", "patterns"),
    ("primary_constructors", "primary-constructors"),
    ("private_named_parameters", "private-named-parameters"),
    ("records", "records"),
    ("sealed_class", "sealed-class"),
    ("set_literals", "set-literals"),
    ("sound_flow_analysis", "sound-flow-analysis"),
    ("spread_collections", "spread-collections"),
    ("this_promotion", "this-promotion"),
    ("variance", "variance"),
    ("wildcard_variables", "wildcard-variables"),
];

/// co19 `LanguageFeatures` directory -> feature flag; unlisted directories
/// are core language behavior (`_core/co19/<Feature>`).
pub const CO19_FLAG_BY_DIR: &[(&str, &str)] = &[
    ("augmentations", "augmentations"),
    ("class-modifiers", "class-modifiers"),
    ("constant-update-2018", "constant-update-2018"),
    ("constructor-tear-offs", "constructor-tearoffs"),
    ("control-flow-collections", "control-flow-collections"),
    ("digit-separators", "digit-separators"),
    ("enhanced-enum", "enhanced-enums"),
    ("extension-methods", "extension-methods"),
    ("extension-types", "inline-class"),
    ("generic-functions-as-type-args", "generic-metadata"),
    ("horizontal-inference", "inference-update-1"),
    ("named-arguments-anywhere", "named-arguments-anywhere"),
    ("nnbd", "non-nullable"),
    ("parts-with-imports", "enhanced-parts"),
    ("patterns", "patterns"),
    ("primary-constructors", "primary-constructors"),
    ("private-named-parameters", "private-named-parameters"),
    ("records", "records"),
    ("set-literals", "set-literals"),
    ("spread-collections", "spread-collections"),
    ("static-access-shorthand", "dot-shorthands"),
    ("static-extensions", "static-extensions"),
    ("super-parameters", "super-parameters"),
    ("triple-shift", "triple-shift"),
    ("unquoted-imports", "unquoted-imports"),
    ("wildcards", "wildcard-variables"),
];

/// The SDK-internal packages every suite case declares, mirroring the
/// official suite's package config: `expect` and `async_helper` are the
/// suites' assertion libraries (`async_helper` re-exports parts of `expect`).
pub const SUITE_PACKAGES_ALWAYS: [&str; 2] = ["expect", "async_helper"];

/// SDK-internal packages declared only by the cases whose imports reference
/// them (every `package:<name>/` import observed across both suites at this
/// pin; the mocks tree provides each of them).
const SUITE_PACKAGES_BY_IMPORT: [&str; 4] = ["meta", "ffi", "collection", "dart_internal"];

/// Collects the `package:` names of the import/export directives of a file,
/// in source order, deduplicated. Directive-anchored like the relative
/// collector, so package names inside message text never match.
pub fn package_imports(content: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with("import ") && !trimmed.starts_with("export ") {
            continue;
        }
        let mut rest = trimmed;
        while let Some(at) = rest.find("package:") {
            let after = &rest[at + "package:".len()..];
            let len = after
                .bytes()
                .take_while(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'_')
                .count();
            let name = &after[..len];
            if len == 0 || !after[len..].starts_with('/') {
                rest = after;
                continue;
            }
            if !found.iter().any(|n| n == name) {
                found.push(name.to_string());
            }
            rest = after;
        }
    }
    found
}

/// A skipped case and why.
#[derive(Debug, PartialEq, Eq)]
pub struct Skip {
    pub id: String,
    pub reason: String,
}

/// Everything extracted from one suite.
#[derive(Debug, Default)]
pub struct FeatureExtraction {
    pub cases: Vec<Case>,
    pub skips: Vec<Skip>,
    /// Test files scanned (each yields one or more cases).
    pub scanned: usize,
}

/// The flag segment for a suite-relative path: the mapped flag of the first
/// path component, else `_core/<dir>` (`_core` for root-level files).
/// co19 directories are CamelCase-hyphenated, so its lookup is
/// case-insensitive while keeping the original name in the fallback.
fn flag_of(rel: &str, table: &[(&str, &str)], core_prefix: &str, fold_case: bool) -> String {
    let first = rel.split('/').next().unwrap_or("");
    let hit = table.iter().find(|(dir, _)| {
        if fold_case {
            dir.eq_ignore_ascii_case(first)
        } else {
            *dir == first
        }
    });
    match hit {
        Some((_, flag)) => (*flag).to_string(),
        None => {
            if first.is_empty() || !rel.contains('/') {
                "_core".to_string()
            } else {
                format!("{core_prefix}/{first}")
            }
        }
    }
}

pub fn language_flag(rel: &str) -> String {
    flag_of(rel, LANGUAGE_FLAG_BY_DIR, "_core", false)
}

pub fn co19_flag(rel: &str) -> String {
    flag_of(rel, CO19_FLAG_BY_DIR, "_core/co19", true)
}

/// Whether a file name is a test of its suite: the language suite ends test
/// files with `_test.dart`, co19 names them `<name>_t<n>.dart`.
pub fn is_language_test(name: &str) -> bool {
    name.ends_with("_test.dart")
}

pub fn is_co19_test(name: &str) -> bool {
    let Some(name) = name.strip_suffix(".dart") else {
        return false;
    };
    match name.rfind("_t") {
        Some(at) => {
            let digits = &name[at + 2..];
            !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
        }
        None => false,
    }
}

/// Extracts one suite. `root` is the suite root on disk (imports resolve
/// against it, so helpers above the test directory are carried too); `scan`
/// is the subdirectory walked for test files (empty for the whole root);
/// `prefix` is the section-path prefix (`language` or `co19`).
pub fn extract_suite(
    root: &Path,
    scan: &str,
    prefix: &str,
    is_test: &dyn Fn(&str) -> bool,
    flag: &dyn Fn(&str) -> String,
) -> FeatureExtraction {
    let mut files = Vec::new();
    collect_tests(root, scan, is_test, &mut files);
    files.sort();
    let mut out = FeatureExtraction::default();
    for rel in &files {
        extract_one(root, prefix, rel, flag, &mut out);
        out.scanned += 1;
    }
    out
}

fn collect_tests(root: &Path, rel: &str, is_test: &dyn Fn(&str) -> bool, out: &mut Vec<String>) {
    let dir = if rel.is_empty() {
        root.to_path_buf()
    } else {
        root.join(rel)
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let child = if rel.is_empty() {
            name.clone()
        } else {
            format!("{rel}/{name}")
        };
        if entry.path().is_dir() {
            collect_tests(root, &child, is_test, out);
        } else if is_test(&name) {
            out.push(child);
        }
    }
}

fn extract_one(
    root: &Path,
    prefix: &str,
    rel: &str,
    flag: &dyn Fn(&str) -> String,
    out: &mut FeatureExtraction,
) {
    let Ok(content) = std::fs::read_to_string(root.join(rel)) else {
        out.skips.push(Skip {
            id: rel.to_string(),
            reason: "unreadable".into(),
        });
        return;
    };
    let (language, experiments) = header_options(&content);
    let flag = flag(rel);
    let options = setup::analysis_options_content(&experiments, &[], false, false, false, &[], &[]);

    // Companion files carried for relative imports (section path, content).
    let mut companions: Vec<(String, String)> = Vec::new();
    carry_imports(
        root,
        prefix,
        rel,
        &content,
        &mut companions,
        &mut Vec::new(),
        out,
    );

    // Package declarations: the suite config's always-present packages plus
    // every SDK-internal package any file of the case imports. A directive
    // naming a package outside the table would ship a broken import graph;
    // such a case is suppressed with a named skip.
    let mut imported = package_imports(&content);
    for (_, text) in &companions {
        for name in package_imports(text) {
            imported.push(name);
        }
    }
    let mut packages: Vec<String> = SUITE_PACKAGES_ALWAYS
        .iter()
        .map(|s| s.to_string())
        .collect();
    for name in &imported {
        if SUITE_PACKAGES_ALWAYS.contains(&name.as_str()) {
            continue;
        }
        if !SUITE_PACKAGES_BY_IMPORT.contains(&name.as_str()) {
            out.skips.push(Skip {
                id: rel.to_string(),
                reason: format!("undeclared_package:{name}"),
            });
            return;
        }
        if !packages.contains(name) {
            packages.push(name.clone());
        }
    }

    let (sections, multitest) = split_multitest(&content, rel);
    for (key, text) in &sections {
        let section_file = if multitest {
            format!("{}_{}.dart", trim_dart(rel), key)
        } else {
            rel.to_string()
        };
        let mut files = vec![(format!("{prefix}/{section_file}"), text.clone())];
        files.extend(companions.iter().cloned());
        let provenance = if multitest {
            format!("tests/{prefix}/{rel}#multitest:{key}")
        } else {
            format!("tests/{prefix}/{rel}")
        };
        out.cases.push(Case {
            id: format!("features/{flag}/{prefix}/{}", trim_dart(&section_file)),
            source: provenance,
            language: language.clone(),
            packages: packages.clone(),
            aux_packages: Vec::new(),
            options: Some(options.clone()),
            ignores: Vec::new(),
            files,
        });
    }
}

fn trim_dart(rel: &str) -> &str {
    rel.strip_suffix(".dart").unwrap_or(rel)
}

/// Reads the language-version override and the SharedOptions experiments of
/// a test file: `// @dart = X.Y` and `// SharedOptions=--enable-experiment=...`.
pub fn header_options(content: &str) -> (Option<String>, Vec<String>) {
    let mut language = None;
    let mut experiments: Vec<String> = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim_start();
        let Some(rest) = trimmed.strip_prefix("//") else {
            continue;
        };
        let rest = rest.trim_start();
        if language.is_none() {
            if let Some(rest) = rest.strip_prefix("@dart") {
                let rest = rest.trim_start();
                if let Some(rest) = rest.strip_prefix('=') {
                    let version = rest.trim();
                    let valid = version.split_once('.').is_some_and(|(a, b)| {
                        !a.is_empty()
                            && a.bytes().all(|b| b.is_ascii_digit())
                            && !b.is_empty()
                            && b.bytes().all(|b| b.is_ascii_digit())
                    });
                    if valid {
                        language = Some(version.to_string());
                    }
                }
            }
        }
        if let Some(rest) = rest.strip_prefix("SharedOptions") {
            let rest = rest.trim_start_matches(['=', ' ']);
            for part in rest.split_whitespace() {
                if let Some(list) = part.strip_prefix("--enable-experiment=") {
                    for flag in list.split(',') {
                        let flag = flag.trim();
                        if !flag.is_empty() && !experiments.iter().any(|e| e == flag) {
                            experiments.push(flag.to_string());
                        }
                    }
                }
            }
        }
    }
    (language, experiments)
}

/// One multitest annotation: `//# key: outcomes`.
fn annotation(line: &str) -> Option<(String, String)> {
    let idx = line.find("//#")?;
    let parts: Vec<&str> = line[idx + 3..]
        .split(':')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if parts.len() <= 1 {
        return None;
    }
    Some((parts[0].to_string(), parts[1].to_string()))
}

/// The valid multitest outcomes.
const OUTCOMES: [&str; 5] = [
    "ok",
    "syntax error",
    "compile-time error",
    "runtime error",
    "static type warning",
];

/// Splits a multitest into its sections (`(key, text)` in key order of first
/// appearance, `none` first). A non-multitest file yields a single `none`
/// section with its content unchanged. Sections whose key never gains a
/// valid outcome are dropped, like the official generator. The boolean
/// reports whether the input actually was a multitest.
pub fn split_multitest(contents: &str, source_path: &str) -> (Vec<(String, String)>, bool) {
    let is_multitest = contents.lines().any(|l| annotation(l).is_some());
    if !is_multitest {
        return (vec![("none".to_string(), contents.to_string())], false);
    }
    let sep = match contents.find('\n') {
        Some(0) => "\n",
        Some(n) if contents.as_bytes()[n - 1] == b'\r' => "\r\n",
        _ => "\n",
    };
    let mut lines: Vec<&str> = contents.split(sep).collect();
    if lines.last() == Some(&"") {
        lines.pop();
    }

    let mut sections: Vec<(String, Vec<String>)> = vec![("none".to_string(), Vec::new())];
    let mut outcomes: Vec<(String, BTreeSet<String>)> = Vec::new();
    for line in &lines {
        let Some((key, rest)) = annotation(line) else {
            for (_, v) in sections.iter_mut() {
                v.push(line.to_string());
            }
            continue;
        };
        if !sections.iter().any(|(k, _)| k == &key) {
            sections.push((key.clone(), sections[0].1.clone()));
        }
        if rest != "continued" && !outcomes.iter().any(|(k, _)| k == &key) {
            outcomes.push((key.clone(), BTreeSet::new()));
        }
        for (k, v) in sections.iter_mut() {
            if k == &key {
                v.push(line.to_string());
                if rest != "continued" {
                    let (_, set) = outcomes
                        .iter_mut()
                        .find(|(ok, _)| ok == &key)
                        .expect("outcome entry exists");
                    for o in rest.split(',').map(str::trim) {
                        if OUTCOMES.contains(&o) {
                            set.insert(o.to_string());
                        }
                    }
                }
            } else {
                v.push(String::new());
            }
        }
    }

    let marker = format!("// Test created from multitest named {source_path}.{sep}");
    let mut out = Vec::new();
    for (key, mut text) in sections {
        if key != "none" && !outcomes.iter().any(|(k, s)| k == &key && !s.is_empty()) {
            continue; // no valid expectation: dropped like the official generator
        }
        text.push(marker.clone());
        out.push((key, text.join(sep)));
    }
    (out, true)
}

/// Collects the relative `import`/`part` paths of a file, in source order,
/// with the official collector's semantics: per line, the first directive
/// with a non-package, non-absolute, non-data URI; paths containing
/// `nonexistent` are skipped (deliberately broken imports); deduplicated.
pub fn relative_imports(content: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for line in content.lines() {
        let Some(path) = line_import(line) else {
            continue;
        };
        if path.contains("nonexistent") {
            continue;
        }
        if !found.iter().any(|p| p == path) {
            found.push(path.to_string());
        }
    }
    found
}

/// The relative path of the first `import`/`part` directive on the line.
/// The official collector's pattern is line-anchored: the directive must
/// open the line, optionally after an `@...` metadata annotation.
fn line_import(line: &str) -> Option<&str> {
    let bytes = line.as_bytes();
    let mut start = 0usize;
    if bytes.first() == Some(&b'@') {
        // Find the directive keyword after the annotation: the first
        // `import`/`part` preceded by whitespace.
        for word in ["import", "part"] {
            let mut from = 0usize;
            while let Some(at) = line[from..].find(word) {
                let at = from + at;
                if at > 0 && bytes[at - 1].is_ascii_whitespace() {
                    start = at;
                    break;
                }
                from = at + word.len();
            }
            if start != 0 {
                break;
            }
        }
        if start == 0 {
            return None;
        }
    }
    for word in ["import", "part"] {
        if !line[start..].starts_with(word) {
            continue;
        }
        let mut p = start + word.len();
        if !bytes.get(p).is_some_and(|b| b.is_ascii_whitespace()) {
            continue;
        }
        while bytes.get(p).is_some_and(|b| b.is_ascii_whitespace()) {
            p += 1;
        }
        let quote = *bytes.get(p)?;
        if quote != b'\'' && quote != b'"' {
            continue;
        }
        let rest = &line[p + 1..];
        if rest.starts_with("dart:")
            || rest.starts_with("dart-ext:")
            || rest.starts_with("data:")
            || rest.starts_with("package:")
            || rest.starts_with('/')
        {
            continue;
        }
        if let Some(end) = rest.find(quote as char) {
            return Some(&rest[..end]);
        }
    }
    None
}

/// Carries the transitive relative imports of a test file as companion
/// sections. All paths resolve against the main test's directory (the
/// official collector resolves every level against the top library).
/// `seen` guards import cycles; a path already in `companions` is skipped.
fn carry_imports(
    root: &Path,
    prefix: &str,
    rel: &str,
    content: &str,
    companions: &mut Vec<(String, String)>,
    seen: &mut Vec<String>,
    out: &mut FeatureExtraction,
) {
    let dir = rel.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
    for imp in relative_imports(content) {
        let resolved = normalize_join(dir, &imp);
        if resolved.starts_with("..") {
            out.skips.push(Skip {
                id: rel.to_string(),
                reason: "helper_import_escape".into(),
            });
            continue;
        }
        if companions.iter().any(|(p, _)| p == &resolved) || seen.contains(&resolved) {
            continue;
        }
        seen.push(resolved.clone());
        let Ok(text) = std::fs::read_to_string(root.join(&resolved)) else {
            out.skips.push(Skip {
                id: rel.to_string(),
                reason: "missing_helper".into(),
            });
            continue;
        };
        carry_imports(root, prefix, &resolved, &text, companions, seen, out);
        companions.push((format!("{prefix}/{resolved}"), text));
    }
}

/// Joins and lexically normalizes a POSIX relative path (`.`/`..` resolved).
fn normalize_join(dir: &str, imp: &str) -> String {
    let joined = format!("{dir}/{imp}");
    let mut parts: Vec<&str> = Vec::new();
    for part in joined.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    parts.join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flag_tables_cover_obvious_dirs() {
        assert_eq!(
            language_flag("dot_shorthands/x_test.dart"),
            "dot-shorthands"
        );
        assert_eq!(language_flag("records/a_test.dart"), "records");
        assert_eq!(language_flag("extension_type/a_test.dart"), "inline-class");
        assert_eq!(language_flag("nnbd/a_test.dart"), "non-nullable");
        assert_eq!(language_flag("abstract/a_test.dart"), "_core/abstract");
        assert_eq!(language_flag("top_level_test.dart"), "_core");
        assert_eq!(
            co19_flag("Static-access-shorthand/x_t01.dart"),
            "dot-shorthands"
        );
        assert_eq!(co19_flag("Extension-types/a_t01.dart"), "inline-class");
        assert_eq!(
            co19_flag("Super-mixins/a_t01.dart"),
            "_core/co19/Super-mixins"
        );
        assert_eq!(co19_flag("regression/x_t01.dart"), "_core/co19/regression");
    }

    #[test]
    fn test_file_detection() {
        assert!(is_language_test("a_test.dart"));
        assert!(!is_language_test("a_lib.dart"));
        assert!(is_co19_test("value_t03.dart"));
        assert!(is_co19_test("A01_t12.dart"));
        assert!(!is_co19_test("tearoffs1.lib.dart"));
        assert!(!is_co19_test("my_fancy_list_lib2.dart"));
        assert!(!is_co19_test("test.package1.dart"));
        assert!(!is_co19_test("application_order_A01_t02_lib2.dart"));
    }

    #[test]
    fn multitest_expansion_matches_official_shape() {
        let src = "aaa\nbbb //# 02: runtime error\nccc //# 02: continued\nddd //# 07: static type warning\neee //# 10: ok\nfff\n";
        let (sections, multitest) = split_multitest(src, "x_test.dart");
        assert!(multitest);
        let get = |k: &str| {
            sections
                .iter()
                .find(|(key, _)| key == k)
                .map(|(_, t)| t.clone())
        };
        assert_eq!(sections.len(), 4);
        // Four tagged lines leave four blank lines in the `none` section.
        assert_eq!(
            get("none").expect("none"),
            "aaa\n\n\n\n\nfff\n// Test created from multitest named x_test.dart.\n"
        );
        assert_eq!(
            get("02").expect("02"),
            "aaa\nbbb //# 02: runtime error\nccc //# 02: continued\n\n\nfff\n// Test created from multitest named x_test.dart.\n"
        );
        assert!(sections.iter().any(|(k, _)| k == "07"));
        assert!(sections.iter().any(|(k, _)| k == "10"));
    }

    #[test]
    fn multitest_sections_without_outcome_are_dropped() {
        let src = "a\nb //# 01: continued\nc //# 02: ok\nd\n";
        let (sections, _) = split_multitest(src, "x.dart");
        // Key 01 never gains a valid outcome; only none and 02 remain.
        assert_eq!(
            sections.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(),
            vec!["none", "02"]
        );
    }

    #[test]
    fn plain_files_pass_through_unchanged() {
        let src = "var x = 1;\n// [analyzer] STATIC_WARNING.X\n";
        let (sections, multitest) = split_multitest(src, "a_test.dart");
        assert!(!multitest);
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].0, "none");
        assert_eq!(sections[0].1, src);
    }

    #[test]
    fn relative_imports_collect_directives() {
        let src = "import 'dart:math';\nimport 'package:x/y.dart';\nimport 'helper.dart';\n@Tags(['a']) import 'annotated.dart';\npart 'parts.dart';\nexport 'exported.dart';\nimport '../upper.dart';\nimport 'nonexistent_file.dart';\nvar part 'not a directive';\n  import 'indented.dart';\n";
        let imports = relative_imports(src);
        // The official pattern is line-anchored: indented and mid-code
        // directives do not match.
        assert_eq!(
            imports,
            vec![
                "helper.dart",
                "annotated.dart",
                "parts.dart",
                "../upper.dart"
            ]
        );
    }

    #[test]
    fn header_options_parse() {
        let src = "// Copyright\n// @dart = 2.19\n// SharedOptions=--enable-experiment=variance,this-promotion -Da=b\nvoid f() {}\n";
        let (language, experiments) = header_options(src);
        assert_eq!(language.as_deref(), Some("2.19"));
        assert_eq!(experiments, vec!["variance", "this-promotion"]);
        let (language, experiments) = header_options("// @dart=3.4\n");
        assert_eq!(language.as_deref(), Some("3.4"));
        assert!(experiments.is_empty());
        assert_eq!(header_options("void f() {}"), (None, Vec::new()));
    }

    #[test]
    fn normalize_join_resolves_dot_dot() {
        assert_eq!(normalize_join("a/b", "../c.dart"), "a/c.dart");
        assert_eq!(normalize_join("a", "b.dart"), "a/b.dart");
        assert_eq!(normalize_join("", "b.dart"), "b.dart");
        assert_eq!(normalize_join("a/b", "../../c.dart"), "c.dart");
    }

    #[test]
    fn package_imports_scan_directives() {
        let src = "import 'package:expect/expect.dart';\nimport \"package:meta/meta.dart\";\n"
            .to_string()
            + "export 'package:ffi/ffi.dart';\n"
            + "// mentions package:not_a_directive in prose\n"
            + "var x = 'package:also_not/inside string';\n"
            + "import 'dart:math';\n";
        assert_eq!(package_imports(&src), vec!["expect", "meta", "ffi"]);
    }

    #[test]
    fn cases_declare_suite_and_imported_packages() {
        let dir = std::env::temp_dir().join(format!("conf-feat-pkg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("feat")).expect("creates");
        std::fs::write(
            dir.join("feat/meta_test.dart"),
            "import 'package:meta/meta.dart';\nvar x = 1;\n",
        )
        .expect("writes");
        std::fs::write(
            dir.join("feat/plain_test.dart"),
            "import 'package:expect/expect.dart';\nvoid f() {}\n",
        )
        .expect("writes");
        let out = extract_suite(&dir, "", "language", &is_language_test, &language_flag);
        let meta = out
            .cases
            .iter()
            .find(|c| c.id.ends_with("meta_test"))
            .expect("meta case");
        assert_eq!(meta.packages, vec!["expect", "async_helper", "meta"]);
        let plain = out
            .cases
            .iter()
            .find(|c| c.id.ends_with("plain_test"))
            .expect("plain case");
        assert_eq!(plain.packages, vec!["expect", "async_helper"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unknown_package_import_suppresses_with_named_skip() {
        let dir = std::env::temp_dir().join(format!("conf-feat-unk-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("feat")).expect("creates");
        std::fs::write(
            dir.join("feat/bad_test.dart"),
            "import 'package:no_such_pkg/x.dart';\nvar x = 1;\n",
        )
        .expect("writes");
        let out = extract_suite(&dir, "", "language", &is_language_test, &language_flag);
        assert!(out.cases.is_empty());
        assert_eq!(
            out.skips,
            vec![Skip {
                id: "feat/bad_test.dart".into(),
                reason: "undeclared_package:no_such_pkg".into(),
            }]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn suite_extraction_carries_helpers_and_expands() {
        let dir = std::env::temp_dir().join(format!("conf-feat-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("feat")).expect("creates");
        std::fs::write(dir.join("feat/part.dart"), "part of 'multi_test.dart';\n").expect("writes");
        std::fs::write(
            dir.join("feat/multi_test.dart"),
            "part 'part.dart';\nvar x = 1;\nmain() { x = 2; } //# 01: compile-time error\n",
        )
        .expect("writes");
        std::fs::write(dir.join("feat/plain_test.dart"), "var ok = 1;\n").expect("writes");
        let out = extract_suite(&dir, "", "language", &is_language_test, &language_flag);
        let mut ids: Vec<&str> = out.cases.iter().map(|c| c.id.as_str()).collect();
        ids.sort();
        assert_eq!(
            ids,
            vec![
                "features/_core/feat/language/feat/multi_test_01",
                "features/_core/feat/language/feat/multi_test_none",
                "features/_core/feat/language/feat/plain_test"
            ],
            "{ids:?}"
        );
        let multi = out
            .cases
            .iter()
            .find(|c| c.id.ends_with("multi_test_none"))
            .expect("multitest");
        assert_eq!(multi.files[0].0, "language/feat/multi_test_none.dart");
        assert!(
            multi
                .files
                .iter()
                .any(|(p, _)| p == "language/feat/part.dart")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
