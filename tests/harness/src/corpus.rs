//! M0 corpus assembly (facts §5 Plan A): filter "self-contained" .dart files from a source
//! directory and copy them into a synthetic package preserving the relative directory layout
//! (zero dependencies, so `dart pub get --offline` passes in one shot).
//! The relative structure must be preserved: flattening would let same-name basenames in
//! subdirectories silently overwrite each other (lessons from facts §7.2).

use std::fs;
use std::path::Path;
use std::time::Duration;

use crate::oracle::{driver_timeout_from_env, run_captured};

/// Corpus package manifest. The sdk lower bound pins the languageVersion pub
/// records in `.dart_tool/package_config.json`; it must stay >= 3.13 (Plan A,
/// docs/facts/version-skew.md §4): a lower pin re-disables shipped language
/// features (e.g. inline-class @ 3.3) and floods the gate with
/// `experiment_not_enabled` assembly noise (51/51 of the old syntactic bucket).
const PUBSPEC: &str = "name: corpus_m0\nenvironment:\n  sdk: ^3.13.0\n";

/// Minimum corpus language version (Plan A, docs/facts/version-skew.md §4).
const MIN_LANGUAGE_VERSION: (u32, u32) = (3, 13);

/// Preflight before oracle runs: the corpus package's own `languageVersion` in
/// `.dart_tool/package_config.json` must be >= [`MIN_LANGUAGE_VERSION`]. The
/// corpus package is the entry whose `rootUri` points at the package root
/// (`"../"` as pub writes it, trailing slash tolerated) relative to
/// `.dart_tool/`. A missing config means stale assembly; a stale version
/// re-enables the version-skew noise class — both fail fast with remediation.
pub(crate) fn check_language_version(corpus_dir: &Path) -> Result<(), String> {
    let config_path = corpus_dir.join(".dart_tool").join("package_config.json");
    let text = fs::read_to_string(&config_path).map_err(|_| {
        format!(
            "preflight: {} not found; run `dart pub get` in corpus `{}`",
            config_path.display(),
            corpus_dir.display()
        )
    })?;
    let config: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("preflight: parsing {}: {e}", config_path.display()))?;
    let packages = config["packages"]
        .as_array()
        .ok_or_else(|| format!("preflight: {} has no packages array", config_path.display()))?;
    let is_self = |p: &serde_json::Value| {
        p["rootUri"].as_str().map(|uri| uri.trim_end_matches('/')) == Some("..")
    };
    let version = packages.iter().find(|p| is_self(p)).ok_or_else(|| {
        format!(
            "preflight: {} has no self entry (rootUri \"../\")",
            config_path.display()
        )
    })?["languageVersion"]
        .as_str()
        .ok_or_else(|| {
            format!(
                "preflight: self entry in {} has no languageVersion",
                config_path.display()
            )
        })?;
    let parsed = parse_language_version(version).ok_or_else(|| {
        format!(
            "preflight: unparseable languageVersion `{version}` in {}",
            config_path.display()
        )
    })?;
    if parsed < MIN_LANGUAGE_VERSION {
        return Err(format!(
            "preflight: corpus `{}` languageVersion is {version}, need >= 3.13; \
             bump the pubspec sdk lower bound and re-run `dart pub get` \
             (docs/facts/version-skew.md §4)",
            corpus_dir.display()
        ));
    }
    Ok(())
}

/// `"3.13"` / `"3.13.0"` / `"3.14"` → (major, minor[, patch]) tuple prefix;
/// `None` = not numeric.
fn parse_language_version(version: &str) -> Option<(u32, u32)> {
    let mut parts = version.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    Some((major, minor))
}

pub(crate) struct CorpusReport {
    pub(crate) files: usize,
    pub(crate) loc: usize,
    pub(crate) skipped: usize,
}

/// Assemble the corpus. Skips copying when the target is already a complete package (the report
/// counts existing files); with `rebuild = true`, wipes lib/ first and re-copies everything.
pub(crate) fn build_corpus(
    source: &Path,
    target: &Path,
    rebuild: bool,
) -> Result<CorpusReport, String> {
    let source = source
        .canonicalize()
        .map_err(|e| format!("source dir {}: {e}", source.display()))?;
    let lib = target.join("lib");
    let complete = target.join("pubspec.yaml").is_file() && lib.is_dir();

    if rebuild || !complete {
        if rebuild && lib.exists() {
            fs::remove_dir_all(&lib).map_err(|e| format!("removing {}: {e}", lib.display()))?;
        }
        fs::create_dir_all(&lib).map_err(|e| format!("creating {}: {e}", lib.display()))?;
        let mut stats = Stats::default();
        copy_tree(&source, &lib, &mut stats)?;
        fs::write(target.join("pubspec.yaml"), PUBSPEC)
            .map_err(|e| format!("writing pubspec: {e}"))?;
        return Ok(CorpusReport {
            files: stats.files,
            loc: stats.loc,
            skipped: stats.skipped,
        });
    }

    // Reuse the existing corpus: re-count so the report matches what is on disk
    let mut stats = Stats::default();
    count_tree(&lib, &mut stats)?;
    Ok(CorpusReport {
        files: stats.files,
        loc: stats.loc,
        skipped: 0,
    })
}

/// Run `dart pub get --offline` in the corpus directory under the shared
/// driver timeout budget (facts §5: zero dependencies, offline, one shot).
pub(crate) fn pub_get(dart: &str, target: &Path) -> Result<(), String> {
    run_pub_get(dart, target, driver_timeout_from_env()?)
}

/// [`pub_get`] with an injected timeout (test seam only; `dart` is the real
/// executable path). A timeout or launch failure keeps the [`run_captured`]
/// detail and gains the corpus dir for context; a non-zero exit keeps the existing
/// stderr-dump format.
fn run_pub_get(dart: &str, target: &Path, timeout: Duration) -> Result<(), String> {
    let args = vec![
        "pub".to_string(),
        "get".to_string(),
        "--offline".to_string(),
    ];
    let out = run_captured("pub_get", dart, &args, Some(target), timeout)
        .map_err(|e| format!("`{dart} pub get --offline` in {}: {e}", target.display()))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(format!(
            "`{dart} pub get --offline` failed in {}:\n{}",
            target.display(),
            stderr
        ));
    }
    Ok(())
}

#[derive(Default)]
struct Stats {
    files: usize,
    loc: usize,
    skipped: usize,
}

#[allow(clippy::naive_bytecount)] // naive wc -l-style counting, performance-insensitive (corpus ~200 files)
fn copy_tree(dir: &Path, dest: &Path, stats: &mut Stats) -> Result<(), String> {
    for entry in sorted_entries(dir)? {
        // lstat semantics: a symlink never matches is_dir/is_file below, so
        // symlinked sources are neither copied nor descended.
        let ty = entry
            .file_type()
            .map_err(|e| format!("reading {}: {e}", entry.path().display()))?;
        let name = entry.file_name();
        if ty.is_dir() {
            if name == ".dart_tool" {
                continue;
            }
            let sub = dest.join(&name);
            fs::create_dir_all(&sub).map_err(|e| format!("creating {}: {e}", sub.display()))?;
            copy_tree(&entry.path(), &sub, stats)?;
        } else if ty.is_file() && entry.path().extension().is_some_and(|e| e == "dart") {
            let bytes = fs::read(entry.path())
                .map_err(|e| format!("reading {}: {e}", entry.path().display()))?;
            let text = std::str::from_utf8(&bytes)
                .map_err(|e| format!("{}: invalid UTF-8: {e}", entry.path().display()))?;
            if !is_self_contained(text) {
                stats.skipped += 1;
                continue;
            }
            let target_file = dest.join(&name);
            fs::write(&target_file, &bytes)
                .map_err(|e| format!("writing {}: {e}", target_file.display()))?;
            stats.files += 1;
            stats.loc += bytes.iter().filter(|&&b| b == b'\n').count();
        } else if ty.is_symlink() && symlink_is_skippable(&entry) {
            // A symlinked .dart file or a symlinked dir would have been a copy/
            // descent candidate; both drivers must see identical corpora, so the
            // link is dropped (copying it could escape or duplicate content) —
            // accounted here, never silently.
            stats.skipped += 1;
        }
    }
    Ok(())
}

/// Whether a symlink entry belongs to a would-be-copied class (`.dart` file or
/// resolved dir): only those are counted in `skipped`; other symlinks are
/// ignored like their plain non-`.dart` counterparts.
fn symlink_is_skippable(entry: &std::fs::DirEntry) -> bool {
    entry.path().extension().is_some_and(|e| e == "dart") || entry.path().is_dir()
}

#[allow(clippy::naive_bytecount)] // same as copy_tree
fn count_tree(dir: &Path, stats: &mut Stats) -> Result<(), String> {
    for entry in sorted_entries(dir)? {
        let ty = entry
            .file_type()
            .map_err(|e| format!("reading {}: {e}", entry.path().display()))?;
        if ty.is_dir() {
            count_tree(&entry.path(), stats)?;
        } else if ty.is_file() && entry.path().extension().is_some_and(|e| e == "dart") {
            let bytes = fs::read(entry.path())
                .map_err(|e| format!("reading {}: {e}", entry.path().display()))?;
            stats.files += 1;
            stats.loc += bytes.iter().filter(|&&b| b == b'\n').count();
        }
    }
    Ok(())
}

fn sorted_entries(dir: &Path) -> Result<Vec<std::fs::DirEntry>, String> {
    let mut entries: Vec<_> = fs::read_dir(dir)
        .map_err(|e| format!("reading dir {}: {e}", dir.display()))?
        .collect::<Result<_, _>>()
        .map_err(|e| format!("reading dir {}: {e}", dir.display()))?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    Ok(entries)
}

/// Self-contained rule (facts §5): the full text contains no `package:` substring (equivalent to `grep -L 'package:'`),
/// and the URIs of all import/export/part directives (if any) are `dart:*`.
/// Each line has `//` line comments stripped before scanning directives; extreme forms such as a `//` (URL)
/// inside a string on a directive line degrade to a "no-URI directive" pass -- even a missed or wrong
/// inclusion is only differential noise (seen by both sides) and does not break the differential comparison.
fn is_self_contained(text: &str) -> bool {
    if text.contains("package:") {
        return false;
    }
    text.lines().all(
        |raw| match directive_uri(strip_line_comment(raw).trim_start()) {
            // Non-`dart:` URI (package:/relative/http:) -> not self-contained
            Some(uri) => uri.starts_with("dart:"),
            None => true,
        },
    )
}

/// Directive line -> `Some(uri)` (directive with a URI); `None` = not a directive line or a directive without a URI (`part of foo;`).
fn directive_uri(line: &str) -> Option<&str> {
    let keyword = ["import", "export", "part"].into_iter().find(|kw| {
        line.starts_with(kw)
            && line[kw.len()..].starts_with(|c: char| c.is_whitespace() || c == '\'')
    })?;
    let rest = &line[keyword.len()..];
    let quote = rest.find(['\'', '"'])?;
    let quote_char = rest.as_bytes()[quote] as char;
    let after = &rest[quote + 1..];
    let end = after.find(quote_char)?;
    Some(&after[..end])
}

fn strip_line_comment(line: &str) -> &str {
    match line.find("//") {
        Some(i) => &line[..i],
        None => line,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn write(dir: &Path, rel: &str, content: &str) {
        let p: PathBuf = dir.join(rel);
        fs::create_dir_all(p.parent().unwrap()).expect("mkdir");
        fs::write(p, content).expect("write");
    }

    #[test]
    fn builds_package_preserving_structure() {
        let root = std::env::temp_dir().join(format!("dq_corpus_{}", std::process::id()));
        let src = root.join("src");
        let tgt = root.join("tgt");
        write(&src, "keep/plain.dart", "void f() {}\n");
        write(
            &src,
            "keep/sub/dart_only.dart",
            "import 'dart:math';\nvoid g() {}\n",
        );
        write(&src, "drop/pkg.dart", "import 'package:x/y.dart';\n");
        write(&src, "drop/rel.dart", "import './other.dart';\n");
        write(&src, "drop/part_uri.dart", "part 'p.dart';\n");
        // `package:` inside a comment is excluded too (matching the facts' grep -L semantics)
        write(
            &src,
            "drop/commented.dart",
            "// see package:foo\nvoid h() {}\n",
        );
        write(&src, "ignored.txt", "not dart\n");

        let report = build_corpus(&src, &tgt, true).expect("build");
        assert_eq!(report.files, 2);
        assert_eq!(report.skipped, 4);
        assert_eq!(report.loc, 3);

        // Relative structure preserved (keep/ subdirectory levels not flattened) + pubspec generated
        assert!(tgt.join("lib").join("keep").join("plain.dart").is_file());
        assert!(
            tgt.join("lib")
                .join("keep")
                .join("sub")
                .join("dart_only.dart")
                .is_file()
        );
        assert_eq!(
            fs::read_to_string(tgt.join("pubspec.yaml")).expect("pubspec"),
            PUBSPEC
        );

        // Rebuild semantics: --rebuild wipes and re-copies; counts unchanged
        fs::write(tgt.join("lib").join("stale.dart"), "junk\n").expect("stale");
        let report = build_corpus(&src, &tgt, true).expect("rebuild");
        assert_eq!(report.files, 2);
        assert!(
            !tgt.join("lib").join("stale.dart").exists(),
            "rebuild wipes stale files"
        );

        // Reuse semantics: a complete package skips copying; stats reflect what exists
        let report = build_corpus(&src, &tgt, false).expect("reuse");
        assert_eq!(report.files, 2);
        assert_eq!(report.skipped, 0);

        fs::remove_dir_all(&root).ok();
    }

    /// `file_type()` is lstat semantics, so a symlinked `.dart` source is
    /// invisible to the copy branch and a symlinked dir is invisible to the
    /// descent branch. Both must be dropped (both drivers must see identical
    /// corpora; copying a link could escape or duplicate content) but counted
    /// in `skipped` -- never silently. Symlinks to non-`.dart` files are
    /// ignored like their plain counterparts.
    #[cfg(unix)]
    #[test]
    fn symlinks_are_neither_copied_nor_descended_but_counted_skipped() {
        let root = std::env::temp_dir().join(format!("dq_symlink_{}", std::process::id()));
        let src = root.join("src");
        let tgt = root.join("tgt");
        write(&src, "keep/plain.dart", "void f() {}\n");
        write(&src, "keep/nested/inner.dart", "var x = 1;\n");
        std::os::unix::fs::symlink(src.join("keep/plain.dart"), src.join("linked.dart"))
            .expect("symlinked .dart file");
        std::os::unix::fs::symlink(src.join("keep/nested"), src.join("linked_dir"))
            .expect("symlinked dir");
        std::os::unix::fs::symlink(src.join("keep/plain.dart"), src.join("notes.txt"))
            .expect("symlinked non-dart file");

        let report = build_corpus(&src, &tgt, true).expect("build");
        assert_eq!(report.files, 2, "real .dart files copied");
        assert_eq!(
            report.skipped, 2,
            "symlinked .dart file and symlinked dir accounted as skipped"
        );
        assert!(
            !tgt.join("lib").join("linked.dart").exists(),
            "symlinked source not copied"
        );
        assert!(
            !tgt.join("lib").join("linked_dir").exists(),
            "symlinked dir not descended"
        );

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn self_contained_rules() {        assert!(is_self_contained("void f() {}"));
        assert!(is_self_contained(
            "import 'dart:async';\nexport 'dart:math' show max;"
        ));
        assert!(!is_self_contained("import 'package:x/y.dart';"));
        assert!(!is_self_contained("export 'rel.dart';"));
        assert!(!is_self_contained("part 'p.dart';"));
        assert!(!is_self_contained(
            "import 'package:x/y.dart'; // dart:core"
        ));
        // Line comment stripping: a commented-out relative import no longer causes a false rejection
        assert!(is_self_contained("// import 'rel.dart';\nvoid f() {}"));
        // `part of foo;` (old URI-less form) matches the facts' grep semantics: pass
        assert!(is_self_contained("part of foo;"));
        // An identifier starting with a keyword is not a directive
        assert!(is_self_contained("imported.process();"));
    }

    fn write_config(dir: &Path, language_version: &str) {
        let config = format!(
            r#"{{"configVersion":2,"packages":[{{"name":"c","rootUri":"../","packageUri":"lib/","languageVersion":"{language_version}"}}],"generator":"pub"}}"#
        );
        write(dir, ".dart_tool/package_config.json", &config);
    }

    /// Preflight: missing config, stale version and malformed config fail with
    /// remediation; >= 3.13 (patch tolerated) passes.
    #[test]
    fn language_version_preflight() {
        let root = std::env::temp_dir().join(format!("dq_preflight_{}", std::process::id()));
        let corpus = root.join("corpus");
        fs::create_dir_all(corpus.join("lib")).expect("mkdir");

        let err = check_language_version(&corpus).expect_err("missing config must fail");
        assert!(err.contains("dart pub get"), "remediation mentioned: {err}");

        write_config(&corpus, "3.0");
        let err = check_language_version(&corpus).expect_err("stale pin must fail");
        assert!(
            err.contains("3.0") && err.contains("3.13"),
            "values cited: {err}"
        );

        write_config(&corpus, "3.12.9");
        assert!(check_language_version(&corpus).is_err(), "3.12.9 < 3.13");

        for ok in ["3.13", "3.13.0", "3.14"] {
            write_config(&corpus, ok);
            check_language_version(&corpus)
                .unwrap_or_else(|e| panic!("languageVersion {ok} must pass: {e}"));
        }

        // No self entry (rootUri != "..") = malformed assembly, fail loudly.
        let foreign = r#"{"configVersion":2,"packages":[{"name":"other","rootUri":"../x","languageVersion":"3.13"}]}"#;
        write(&corpus, ".dart_tool/package_config.json", foreign);
        assert!(
            check_language_version(&corpus).is_err(),
            "missing self entry must fail"
        );

        // Version entry that is not numeric must fail, not silently pass.
        let junk = r#"{"configVersion":2,"packages":[{"name":"c","rootUri":"../","languageVersion":"latest"}]}"#;
        write(&corpus, ".dart_tool/package_config.json", junk);
        assert!(
            check_language_version(&corpus).is_err(),
            "junk version must fail"
        );

        fs::remove_dir_all(&root).ok();
    }

    /// Version string parsing accepts major.minor[.patch]; garbage is `None`.
    #[test]
    fn language_version_parsing() {
        assert_eq!(parse_language_version("3.13"), Some((3, 13)));
        assert_eq!(parse_language_version("3.13.0"), Some((3, 13)));
        assert_eq!(parse_language_version("3.14"), Some((3, 14)));
        assert_eq!(parse_language_version("3"), None);
        assert_eq!(parse_language_version("latest"), None);
        assert_eq!(parse_language_version("x.y"), None);
    }

    /// A hung `pub get` is killed within the budget (shared driver timeout
    /// semantics) and the error names the command, the corpus dir and the
    /// captured stderr tail.
    #[test]
    fn pub_get_timeout_kills_hung_dart() {
        use std::os::unix::fs::PermissionsExt;
        use std::time::Instant;

        let root = std::env::temp_dir().join(format!("dq_pubget_to_{}", std::process::id()));
        let corpus = root.join("corpus");
        fs::create_dir_all(&corpus).expect("mkdir");
        // Stand-in "dart": ignores the fixed `pub get --offline` args, hangs.
        let slow_dart = root.join("slow_dart.sh");
        fs::write(&slow_dart, "#!/bin/sh\necho boom >&2\nexec sleep 30\n").expect("script");
        fs::set_permissions(&slow_dart, fs::Permissions::from_mode(0o755)).expect("chmod");

        let started = Instant::now();
        // 2s budget: a cold /bin/sh start can exceed a few hundred ms, and a
        // kill landing before `echo` would flake the stderr-tail assertion.
        let err = run_pub_get(
            slow_dart.to_str().expect("utf8 temp path"),
            &corpus,
            Duration::from_secs(2),
        )
        .expect_err("hung pub get must time out");
        assert!(err.contains("timed out"), "error: {err}");
        assert!(err.contains("pub get --offline"), "command named: {err}");
        assert!(err.contains("corpus"), "target dir cited: {err}");
        assert!(err.contains("boom"), "stderr tail attached: {err}");
        assert!(
            started.elapsed() < Duration::from_secs(15),
            "must fail within the test budget, took {:?}",
            started.elapsed()
        );

        fs::remove_dir_all(&root).ok();
    }
}
