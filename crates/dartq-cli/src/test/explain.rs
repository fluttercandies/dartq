//! `dartq test --explain-cache[=text|json]`: why the result cache refuses
//! each suite, without running anything.
//!
//! The report comes from the same keying pass a run makes
//! ([`dartq_test::Cache::keys`]): one closure walk per suite, which keeps
//! the first witness it meets — the refusal kind, the file and line whose
//! read or marker decided it, and the import chain from the suite to that
//! file. The summary ranks the witnesses by how many suites each one
//! refuses: fixing (or attesting) the top entry is what turns the most
//! suites cacheable.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use rustc_hash::FxHashMap;

use dartq_test::{Refusal, RefusalDetail};

use super::{discover, display_path, file_config, flutter_pin, flutter_route, parse};
use crate::exit;

/// How many witnesses the summary ranks.
const TOP: usize = 10;

/// The option that asks for the report, before any `--`.
const FLAG: &str = "--explain-cache";

/// The report's shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Format {
    Text,
    Json,
}

/// Answer the line when it asks for `--explain-cache`; `None` leaves it to
/// the normal run.
pub(super) fn run(args: &[String]) -> Option<u8> {
    let (at, format) = args
        .iter()
        .take_while(|arg| *arg != "--")
        .enumerate()
        .find_map(|(index, arg)| {
            let value = arg.strip_prefix(FLAG)?;
            match value {
                "" | "=text" => Some((index, Ok(Format::Text))),
                "=json" => Some((index, Ok(Format::Json))),
                other => other
                    .strip_prefix('=')
                    .map(|value| (index, Err(value.to_owned()))),
            }
        })?;
    let format = match format {
        Ok(format) => format,
        Err(value) => {
            eprintln!("dartq test: {FLAG} takes 'text' or 'json' (got '{value}')");
            return Some(exit::USAGE);
        }
    };
    let mut rest = args.to_vec();
    rest.remove(at);
    Some(match explain(&rest, format) {
        Ok(()) => exit::CLEAN,
        Err((code, message)) => {
            eprintln!("dartq test {FLAG}: {message}");
            code
        }
    })
}

/// Key every selected suite, then print why each refused one has no key.
fn explain(args: &[String], format: Format) -> Result<(), (u8, String)> {
    let usage = |message: String| (exit::USAGE, message);
    let root = std::env::current_dir()
        .and_then(|dir| dir.canonicalize())
        .map_err(|error| {
            (
                exit::INTERNAL_ERROR,
                format!("cannot resolve the current directory: {error}"),
            )
        })?;
    let flutter = flutter_route(args, &root).is_some();
    let inv = parse(args, flutter).map_err(|()| usage("unrecognized arguments".to_owned()))?;
    if !root.join("pubspec.yaml").is_file() {
        return Err(usage("no pubspec.yaml here".to_owned()));
    }
    // A `dart_test.yaml` only the runner reads still selects suites the
    // usual way: its `paths`/`filename` are what this report honors.
    let config = file_config(&root).unwrap_or_default();
    let selections = if inv.paths.is_empty() {
        config
            .paths
            .clone()
            .unwrap_or_else(|| vec!["test".to_owned()])
    } else {
        inv.paths.clone()
    };
    let suites = discover(&root, &selections, config.filename.as_deref())
        .ok_or_else(|| usage("no test suites discovered".to_owned()))?;
    let bin = if flutter {
        dartq_test::host::find_flutter(flutter_pin().as_deref())
    } else {
        dartq_test::host::find_dart(None)
    }
    .ok_or_else(|| {
        usage(format!(
            "no {} on PATH",
            if flutter { "flutter" } else { "dart" }
        ))
    })?;
    let runner = dartq_test::Runner {
        bin: &bin,
        args: &inv.pass,
        flutter,
    };
    let abs: Vec<PathBuf> = suites.iter().map(|suite| suite.abs.clone()).collect();
    let mut cache = dartq_test::Cache::open(&root);
    cache.keys(&abs, runner).map_err(|error| {
        (
            exit::INTERNAL_ERROR,
            format!("cannot key the suites: {error}"),
        )
    })?;
    let rows: Vec<(&str, &Path)> = suites
        .iter()
        .map(|suite| (suite.spelling.as_str(), suite.abs.as_path()))
        .collect();
    let out = match format {
        Format::Text => text(&root, &rows, cache.refusal_details()),
        Format::Json => json(&root, &rows, cache.refusal_details()),
    };
    print!("{out}");
    Ok(())
}

/// One witness as the summary groups it: the deciding file, its line, and
/// what matched there.
type Witness<'a> = (Option<&'a Path>, Option<u32>, &'a str);

fn witness(detail: &RefusalDetail) -> Witness<'_> {
    (
        detail.file.as_deref(),
        detail.line,
        detail
            .triggers
            .first()
            .map_or(detail.refusal.label(), String::as_str),
    )
}

/// The witnesses ranked by how many suites each refuses, most first (file,
/// line and evidence breaking ties), with the count of each.
fn ranked<'a>(details: &'a [Option<RefusalDetail>]) -> Vec<(Witness<'a>, usize)> {
    let mut counts: FxHashMap<Witness<'a>, usize> = FxHashMap::default();
    for detail in details.iter().flatten() {
        *counts.entry(witness(detail)).or_default() += 1;
    }
    let mut ranked: Vec<(Witness<'a>, usize)> = counts.into_iter().collect();
    ranked.sort_unstable_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    ranked
}

/// The refusal kinds with their suite counts, most first.
fn kinds(details: &[Option<RefusalDetail>]) -> Vec<(&'static str, usize)> {
    let mut counts: FxHashMap<&'static str, usize> = FxHashMap::default();
    for detail in details.iter().flatten() {
        *counts.entry(detail.refusal.label()).or_default() += 1;
    }
    let mut kinds: Vec<(&'static str, usize)> = counts.into_iter().collect();
    kinds.sort_unstable_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    kinds
}

/// `file[:line][ evidence]`, the file relative to `root` (`evidence`
/// alone when the refusal names no file).
fn site(root: &Path, (file, line, evidence): Witness<'_>) -> String {
    let Some(file) = file else {
        return evidence.to_owned();
    };
    let mut out = display_path(root, file);
    if let Some(line) = line {
        let _ = write!(out, ":{line}");
    }
    if !evidence.is_empty() {
        let _ = write!(out, " {evidence}");
    }
    out
}

/// What a refusal kind asks the user to do.
fn remedy(refusal: Refusal) -> &'static str {
    match refusal {
        Refusal::Resolution => "run `dart pub get`",
        Refusal::SuiteUnknown => "the suite is outside the resolved package",
        Refusal::Untrusted => "spell the directive URI as a plain literal",
        Refusal::NoCache => "remove the marker if the suite is deterministic",
        Refusal::Nondeterministic => {
            "inject the source behind a seam, or attest `// dartq: deterministic` in that file"
        }
        Refusal::AbsoluteDirectory => "name the files the suite reads, not a directory",
        Refusal::EnvOpaque => "read environment variables by literal name",
    }
}

/// One line per refused suite, its chain beneath it when the witness is not
/// the suite's own file, then the counts and the ranked witnesses.
fn text(root: &Path, suites: &[(&str, &Path)], details: &[Option<RefusalDetail>]) -> String {
    let mut out = String::new();
    for ((spelling, _), detail) in suites.iter().zip(details) {
        let Some(detail) = detail else {
            continue;
        };
        let _ = write!(out, "{spelling} → {}", detail.refusal.label());
        let evidence = detail.triggers.first().map_or("", String::as_str);
        match &detail.file {
            Some(file) => {
                let _ = write!(out, ": {}", site(root, (Some(file), detail.line, evidence)));
            }
            None if !evidence.is_empty() => {
                let _ = write!(out, ": {evidence}");
            }
            None => {}
        }
        out.push('\n');
        if detail.chain.len() > 1 {
            let chain: Vec<String> = detail
                .chain
                .iter()
                .map(|path| display_path(root, path))
                .collect();
            let _ = writeln!(out, "    via {}", chain.join(" → "));
        }
    }
    let refused = details.iter().flatten().count();
    let _ = write!(out, "{refused} of {} suites refused", suites.len());
    let kinds = kinds(details);
    if !kinds.is_empty() {
        let kinds: Vec<String> = kinds
            .iter()
            .map(|(label, count)| format!("{count} {label}"))
            .collect();
        let _ = write!(out, " ({})", kinds.join(", "));
    }
    out.push('\n');
    let ranked = ranked(details);
    if !ranked.is_empty() {
        let _ = writeln!(out, "top witnesses (suites refused):");
        for (witness, count) in ranked.iter().take(TOP) {
            let _ = writeln!(out, "  {count:>5}  {}", site(root, *witness));
        }
        if ranked.len() > TOP {
            let _ = writeln!(out, "  … and {} more", ranked.len() - TOP);
        }
        let mut seen: Vec<Refusal> = Vec::new();
        for detail in details.iter().flatten() {
            if !seen.contains(&detail.refusal) {
                seen.push(detail.refusal);
            }
        }
        for refusal in seen {
            let _ = writeln!(out, "  {}: {}", refusal.label(), remedy(refusal));
        }
    }
    out
}

/// The same report as one JSON object: `suites`, `refused`, `reasons`
/// (label → count), `files` (one entry per refused suite) and `top` (the
/// ranked witnesses).
fn json(root: &Path, suites: &[(&str, &Path)], details: &[Option<RefusalDetail>]) -> String {
    use serde_json::{Value, json};
    let relative = |path: &Path| Value::String(display_path(root, path));
    let files: Vec<Value> = suites
        .iter()
        .zip(details)
        .filter_map(|((spelling, _), detail)| {
            let detail = detail.as_ref()?;
            Some(json!({
                "suite": spelling,
                "reason": detail.refusal.label(),
                "file": detail.file.as_deref().map(relative),
                "line": detail.line,
                "evidence": detail.triggers,
                "chain": detail.chain.iter().map(|path| relative(path)).collect::<Vec<_>>(),
            }))
        })
        .collect();
    let reasons: serde_json::Map<String, Value> = kinds(details)
        .into_iter()
        .map(|(label, count)| (label.to_owned(), json!(count)))
        .collect();
    let top: Vec<Value> = ranked(details)
        .into_iter()
        .take(TOP)
        .map(|((file, line, evidence), count)| {
            json!({
                "file": file.map(relative),
                "line": line,
                "evidence": evidence,
                "suites": count,
            })
        })
        .collect();
    let report = json!({
        "suites": suites.len(),
        "refused": files.len(),
        "reasons": reasons,
        "files": files,
        "top": top,
    });
    format!("{report}\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detail(
        refusal: Refusal,
        file: Option<&str>,
        line: Option<u32>,
        triggers: &[&str],
        chain: &[&str],
    ) -> Option<RefusalDetail> {
        Some(RefusalDetail {
            refusal,
            file: file.map(PathBuf::from),
            triggers: triggers.iter().map(|t| (*t).to_owned()).collect(),
            line,
            chain: chain.iter().map(PathBuf::from).collect(),
        })
    }

    /// One line per refusal kind: the kind, the deciding file and line
    /// with what matched, and the import chain when the witness lies past
    /// the suite's own file.
    #[test]
    fn explain_text_names_every_refusal_kind() {
        let root = Path::new("/pkg");
        let names: Vec<String> = (0..8).map(|i| format!("test/s{i}_test.dart")).collect();
        let abs: Vec<PathBuf> = names.iter().map(|name| root.join(name)).collect();
        let suites: Vec<(&str, &Path)> = names
            .iter()
            .zip(&abs)
            .map(|(name, abs)| (name.as_str(), abs.as_path()))
            .collect();
        let details = vec![
            None,
            detail(Refusal::Resolution, None, None, &[], &[]),
            detail(
                Refusal::SuiteUnknown,
                Some("/pkg/test/s2_test.dart"),
                None,
                &[],
                &[],
            ),
            detail(
                Refusal::Untrusted,
                Some("/pkg/lib/gen.dart"),
                None,
                &["unresolvable import x:y"],
                &["/pkg/test/s3_test.dart", "/pkg/lib/gen.dart"],
            ),
            detail(
                Refusal::NoCache,
                Some("/pkg/test/s4_test.dart"),
                Some(2),
                &["// dartq: no-cache"],
                &["/pkg/test/s4_test.dart"],
            ),
            detail(
                Refusal::Nondeterministic,
                Some("/pkg/lib/src/net.dart"),
                Some(12),
                &["Process.run"],
                &[
                    "/pkg/test/s5_test.dart",
                    "/pkg/lib/api.dart",
                    "/pkg/lib/src/net.dart",
                ],
            ),
            detail(
                Refusal::AbsoluteDirectory,
                Some("/pkg/test/s6_test.dart"),
                None,
                &["absolute path /etc"],
                &["/pkg/test/s6_test.dart"],
            ),
            detail(
                Refusal::EnvOpaque,
                Some("/pkg/lib/env.dart"),
                Some(3),
                &["Platform.environment (unnamed key)"],
                &["/pkg/test/s7_test.dart", "/pkg/lib/env.dart"],
            ),
        ];
        let out = text(root, &suites, &details);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(
            lines[..10],
            [
                "test/s1_test.dart → resolution",
                "test/s2_test.dart → unknown-suite: test/s2_test.dart",
                "test/s3_test.dart → untrusted: lib/gen.dart unresolvable import x:y",
                "    via test/s3_test.dart → lib/gen.dart",
                "test/s4_test.dart → no-cache: test/s4_test.dart:2 // dartq: no-cache",
                "test/s5_test.dart → nondeterministic: lib/src/net.dart:12 Process.run",
                "    via test/s5_test.dart → lib/api.dart → lib/src/net.dart",
                "test/s6_test.dart → absolute-directory: test/s6_test.dart absolute path /etc",
                "test/s7_test.dart → environment: lib/env.dart:3 Platform.environment (unnamed key)",
                "    via test/s7_test.dart → lib/env.dart",
            ]
        );
        assert!(
            lines[10].starts_with("7 of 8 suites refused ("),
            "{}",
            lines[10]
        );
        assert_eq!(lines[11], "top witnesses (suites refused):");
    }

    /// The summary ranks witnesses by the suites each refuses — the
    /// actionable list — and the JSON form carries the same report.
    #[test]
    fn explain_ranks_witnesses_by_suites_refused() {
        let root = Path::new("/pkg");
        let names = ["test/a_test.dart", "test/b_test.dart", "test/c_test.dart"];
        let abs: Vec<PathBuf> = names.iter().map(|name| root.join(name)).collect();
        let suites: Vec<(&str, &Path)> = names
            .iter()
            .zip(&abs)
            .map(|(name, abs)| (*name, abs.as_path()))
            .collect();
        let net = |suite: &str| {
            detail(
                Refusal::Nondeterministic,
                Some("/pkg/lib/net.dart"),
                Some(4),
                &["Socket.connect"],
                &[suite, "/pkg/lib/net.dart"],
            )
        };
        let details = vec![
            net("/pkg/test/a_test.dart"),
            net("/pkg/test/b_test.dart"),
            detail(
                Refusal::Nondeterministic,
                Some("/pkg/test/c_test.dart"),
                Some(1),
                &["DateTime.now"],
                &["/pkg/test/c_test.dart"],
            ),
        ];
        let out = text(root, &suites, &details);
        let top: Vec<&str> = out
            .lines()
            .skip_while(|line| !line.starts_with("top witnesses"))
            .skip(1)
            .take(2)
            .collect();
        assert_eq!(
            top,
            [
                "      2  lib/net.dart:4 Socket.connect",
                "      1  test/c_test.dart:1 DateTime.now"
            ]
        );
        let value: serde_json::Value =
            serde_json::from_str(&json(root, &suites, &details)).expect("valid json");
        assert_eq!(value["suites"], 3);
        assert_eq!(value["refused"], 3);
        assert_eq!(value["reasons"]["nondeterministic"], 3);
        assert_eq!(value["top"][0]["file"], "lib/net.dart");
        assert_eq!(value["top"][0]["suites"], 2);
        assert_eq!(
            value["files"][0]["chain"],
            serde_json::json!(["test/a_test.dart", "lib/net.dart"])
        );
    }
}
