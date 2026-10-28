//! `dartq impact` — the impact face: what a change to one file
//! would affect, over the analyzed workspace.
//!
//! The substrate (import closure over the engine's CSR, the test graph, the
//! folded file snapshot) and the report assembly live in
//! [`dartq_commands::impact`] — the one implementation the daemon's
//! `dartq/impact` shares. This binary adds argument parsing, the crash
//! tier, the usage-tier mapping, and the JSON output.
//!
//! One-shot posture: a CLI run holds no baseline, so the seed enters the
//! query as a **hypothetical** API-level change — the report answers "what
//! depends on this file's API surface". The daemon passes real before/after
//! diffs through the same query.
//!
//! Output is the frozen JSON contract (`v: 3`; the path columns spell
//! through the one file locator — root-relative inside the analysis root,
//! absolute outside it, the envelope's `root` riding once when a
//! file-bearing row does), schema-valid at birth (the shared core's
//! shaper validates against `Domain::Impact`). The report is
//! tier-invariant (the
//! canonical tables: every field a locator or a verdict — nothing to
//! trim), so `--verbosity` parses for grammar parity and every tier emits
//! the same bytes.

use std::io::{self, BufWriter, Write};
use std::path::PathBuf;

use clap::Args;

use dartq_commands::impact::{ImpactError, impact as query_impact, to_json};
use dartq_commands::workspace::open_engine;

use crate::Verbosity;
use crate::exit;

#[derive(Args)]
pub(crate) struct ImpactArgs {
    /// The changed file to assess; must be an analyzed `.dart` file of the
    /// workspace. A relative path resolves against DIR.
    pub(crate) path: PathBuf,
    /// Package directory holding pubspec.yaml and
    /// `.dart_tool/package_config.json`; defaults to the current directory.
    #[arg(default_value = ".")]
    pub(crate) dir: PathBuf,
    /// Dart SDK root; defaults to `$DART_SDK`.
    #[arg(long)]
    pub(crate) sdk: Option<PathBuf>,
    /// Report detail tier (`-v`/`--verbosity minimal|compact|full`), for
    /// grammar parity with the other faces. The report is tier-invariant
    /// (the canonical tables: every field a locator or a verdict — the
    /// published schema requires them at every tier), so every tier emits
    /// the same bytes; the default `compact` was already the only shape.
    #[arg(short = 'v', long, value_enum, default_value_t = Verbosity::Compact)]
    pub(crate) verbosity: Verbosity,
    /// Narrow the hypothetical-change seed to faces of the target's owning
    /// library. Three spellings: a bare `name` seeds every same-named
    /// declaration (the face `name` itself plus every container declaring
    /// a member or constructor named `name`); `Container.name` seeds that
    /// container's face (the signature fold is per-container, so the
    /// member part narrows nothing past it); and `path/to/file.dart:line`
    /// seeds the declaration whose name token starts on that line of a
    /// seed-library file. Repeatable; the union of the resolved hits
    /// seeds the propagation set the `affected[].reference_lines` collect
    /// against. A spelling that resolves to nothing is a usage error.
    /// Omitting the flag is the file-level posture: every library-face
    /// name of the owning library seeds.
    #[arg(long = "symbol", value_name = "NAME")]
    pub(crate) symbols: Vec<String>,
    /// Output format: `lon` (the default) or `json` (the frozen report,
    /// both the same value — `sym`/`files`/`tests`/`envs`/`lines`/
    /// `sig_changed`), or `text` (the indented report).
    #[arg(long, value_enum, default_value_t = crate::OutputFormat::Lon)]
    pub(crate) format: crate::OutputFormat,
}

/// Crash tier like the other engine faces: a panic aborts with exit 4 and
/// stdout kept empty, because [`run`] writes only after the query fully
/// succeeded.
pub(crate) fn run_entry(args: &ImpactArgs) -> u8 {
    let outcome = crate::crash_tier(|| {
        run(args, &mut BufWriter::new(io::stdout().lock()))
    });
    let Ok(code) = outcome else {
        eprintln!("dartq: internal error: analysis panicked");
        return exit::INTERNAL_ERROR;
    };
    code
}

/// One impact run over a caller-supplied writer (`pub(crate)` for the
/// resident tests' byte pins).
pub(crate) fn run(args: &ImpactArgs, out: &mut impl Write) -> u8 {
    // The tier parses for grammar parity and shapes nothing: the report is
    // tier-invariant (the canonical tables trim nothing here), the same
    // accept-and-document posture the daemon's `dartq/impact` answer holds.
    let _ = args.verbosity;
    // The resident consult (#2629) leads: the session's maintained view
    // answers the same report, rendered through the same writers.
    if let Some(code) = crate::resident::route_impact(args, out) {
        return code;
    }
    let (mut engine, dir) = match open_engine(&args.dir, args.sdk.as_deref()) {
        Ok(faces) => faces,
        Err(error) => {
            eprintln!("error: {error}");
            return exit::USAGE;
        }
    };
    // The target resolves against `--dir`, not the process cwd: a relative
    // path joins the assembly's canonical package directory — the same
    // resolution base the daemon's workspace-root resolution serves. An
    // absolute path passes through (the core canonicalizes either way).
    let target = if args.path.is_absolute() {
        args.path.clone()
    } else {
        dir.join(&args.path)
    };
    let report = match query_impact(&mut engine, &target, &args.symbols) {
        Ok(report) => report,
        // Target problems, plus the environments axis' unusable inputs (a
        // malformed config, an unresolvable context locator), are the usage
        // tier — their Display bodies are the printed messages, the compat
        // face's split ([`ImpactError`]'s contract); prelude and axis-engine
        // failures the internal tier.
        Err(
            error @ (ImpactError::Missing(_) | ImpactError::NotAnalyzed(_) | ImpactError::Usage(_)),
        ) => {
            eprintln!("error: {error}");
            return exit::USAGE;
        }
        Err(error) => {
            eprintln!("dartq: internal error: {error}");
            return exit::INTERNAL_ERROR;
        }
    };

    let written = match args.format {
        crate::OutputFormat::Json => out
            .write_all(
                serde_json::to_string(&to_json(&report))
                    .expect("the validated report serializes")
                    .as_bytes(),
            )
            .and_then(|()| out.write_all(b"\n")),
        crate::OutputFormat::Lon => crate::output::write_lon(out, &to_json(&report)),
        crate::OutputFormat::Text => crate::output::write_text_value(out, &to_json(&report)),
    }
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
    use crate::{Cli, Command};
    use clap::Parser as _;
    use std::fs;

    /// The CLI face's grammar, mirroring the sibling faces' parse tests:
    /// `--sdk` is an option and `path`/`dir` the only positionals — the
    /// shape that preceded the `#[arg(long)]` restore (a bare third
    /// positional silently filling `sdk`) must reject.
    #[test]
    fn impact_args_parse() {
        fn parse(args: &[&str]) -> ImpactArgs {
            match Cli::try_parse_from(std::iter::once("dartq").chain(args.iter().copied()))
                .expect("parse")
                .command
            {
                Some(Command::Impact(args)) => args,
                Some(_) => panic!("expected the impact command for {args:?}"),
                None => panic!("expected a subcommand"),
            }
        }

        let args = parse(&["impact", "lib/a.dart"]);
        assert_eq!(args.path, PathBuf::from("lib/a.dart"));
        assert_eq!(args.dir, PathBuf::from("."), "default target cwd");
        assert!(args.sdk.is_none(), "no sdk by default");
        assert_eq!(
            args.verbosity.into_commands(),
            dartq_commands::Verbosity::Compact,
            "compact is the default tier"
        );

        let args = parse(&["impact", "--sdk", "/sdk", "/pkg/lib/a.dart", "/pkg"]);
        assert_eq!(args.sdk, Some(PathBuf::from("/sdk")), "--sdk parses");
        assert_eq!(args.path, PathBuf::from("/pkg/lib/a.dart"));
        assert_eq!(args.dir, PathBuf::from("/pkg"));

        let args = parse(&["impact", "-v", "minimal", "lib/a.dart"]);
        assert_eq!(
            args.verbosity.into_commands(),
            dartq_commands::Verbosity::Minimal,
            "--verbosity parses"
        );

        for rejected in [
            &["impact", "lib/a.dart", "/pkg", "/sdk"][..],
            &["impact", "--bogus"][..],
        ] {
            assert!(
                Cli::try_parse_from(std::iter::once("dartq").chain(rejected.iter().copied()))
                    .is_err(),
                "must reject {rejected:?}"
            );
        }
    }

    /// The fixture shape of the analyze/query faces' tests: a real package
    /// directory with a stub SDK (`sdk/lib/libraries.json` is the only file
    /// `dartq_sdk::SdkSnapshot::load` needs), a lib pair, and the
    /// convention-placed test that reaches the lib through its package URI.
    fn fixture(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("dq_cli_impact_{tag}_{}", std::process::id()));
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
            // The pub-writer shape: pub always emits `packageUri: "lib/"`
            // (upstream pub source, lib/src/entrypoint.dart:511) — without
            // it the reader contract anchors the source root at the
            // package root itself (package_config_json.dart:152), and no
            // `package:` URI addresses the `lib/` members.
            r#"{"configVersion": 2, "packages": [{"name": "fixture", "rootUri": "../", "packageUri": "lib/", "languageVersion": "3.5"}], "generator": "pub"}"#,
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
        write(
            "lib/b.dart",
            "import 'a.dart';\nvoid use() {\n  Foo? f = Foo();\n  foo();\n}\n",
        );
        write(
            "test/a_test.dart",
            "import 'package:fixture/a.dart';\nimport 'package:test/test.dart';\nvoid main() {\n  test('hits foo', () {\n    foo();\n  });\n}\n",
        );
        root
    }

    /// End to end: the hypothetical change of `lib/a.dart` names its face
    /// symbols, tags the direct importers (`b.dart`, the test) semantic,
    /// and lands the test file in `AffectedTests`.
    #[test]
    fn impact_end_to_end() {
        let root = fixture("e2e");
        let a = root.canonicalize().expect("root").join("lib/a.dart");
        let args = ImpactArgs {
            path: a.clone(),
            dir: root.clone(),
            sdk: Some(root.join("sdk")),
            verbosity: Verbosity::Compact,
            format: crate::OutputFormat::Json,
            symbols: Vec::new(),
        };
        let mut out: Vec<u8> = Vec::new();
        let code = run(&args, &mut out);
        assert_eq!(code, exit::CLEAN);

        let json: serde_json::Value =
            serde_json::from_str(&String::from_utf8(out).expect("UTF-8")).expect("one object");
        assert_eq!(json["v"], 3);
        assert_eq!(
            json["root"],
            root.canonicalize().expect("root").display().to_string(),
            "the envelope carries the analysis root once"
        );
        // Every file column spells through the one file locator
        // (root-relative inside the analysis root), ASCII-sorted,
        // hypothetical signature flags.
        let symbols: Vec<&str> = json["sym"]
            .as_array()
            .expect("sym")
            .iter()
            .map(|row| row["sym"].as_str().expect("sym"))
            .collect();
        assert_eq!(
            symbols,
            vec!["lib/a.dart|Foo", "lib/a.dart|foo",],
            "every face name, ASCII-sorted, hypothetical signature flags"
        );
        assert!(
            json["sym"]
                .as_array()
                .expect("sym")
                .iter()
                .all(|row| row["sig_changed"] == true),
        );

        // The located spellings are root-relative (the lib and test trees
        // alike), and the path sort puts `lib/b.dart` before
        // `test/a_test.dart`.
        let affected: Vec<(String, &str)> = json["files"]
            .as_array()
            .expect("files")
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
                ("lib/b.dart".to_string(), "semantic"),
                ("test/a_test.dart".to_string(), "semantic"),
            ],
            "both import a directly; the import of the test resolves through the package URI"
        );
        let tests: Vec<String> = json["tests"]
            .as_array()
            .expect("tests")
            .iter()
            .map(|row| row["path"].as_str().expect("path").to_string())
            .collect();
        assert_eq!(tests, vec!["test/a_test.dart".to_string()]);
        // The contextless default row is the compat rule's verdict over the
        // affected set, and the set spans the test file: its
        // `package:test` import has no package-config entry, the URI
        // layer's ERROR lands in the set, and the honest row is
        // incompatible.
        assert_eq!(
            json["envs"],
            serde_json::json!([{ "label": "default", "compatible": false }]),
        );

        fs::remove_dir_all(&root).ok();
    }

    /// The tier-invariance boundary, end to end: the report's canonical
    /// tables trim nothing (every field a locator or a verdict the
    /// published schema requires at every tier), so `minimal`, `compact`,
    /// and `full` emit identical bytes.
    #[test]
    fn every_verbosity_tier_emits_identical_bytes() {
        let root = fixture("tiers");
        let base = ImpactArgs {
            path: root.join("lib/a.dart"),
            dir: root.clone(),
            sdk: Some(root.join("sdk")),
            verbosity: Verbosity::Compact,
            symbols: Vec::new(),
            format: crate::OutputFormat::Json,
        };
        let mut compact: Vec<u8> = Vec::new();
        assert_eq!(run(&base, &mut compact), exit::CLEAN);
        for (tier, name) in [(Verbosity::Minimal, "minimal"), (Verbosity::Full, "full")] {
            let args = ImpactArgs {
                verbosity: tier,
                path: base.path.clone(),
                dir: base.dir.clone(),
                sdk: base.sdk.clone(),
                symbols: Vec::new(),
                format: crate::OutputFormat::Json,
            };
            let mut out: Vec<u8> = Vec::new();
            assert_eq!(run(&args, &mut out), exit::CLEAN);
            assert_eq!(
                out, compact,
                "{name} emits the compact bytes: nothing to trim"
            );
        }

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
            verbosity: Verbosity::Compact,
            format: crate::OutputFormat::Json,
            symbols: Vec::new(),
        };
        assert_eq!(run(&args, &mut Vec::new()), exit::USAGE);
        fs::remove_dir_all(&root).ok();
    }

    /// A relative target resolves against `--dir`, not the process cwd: the
    /// fixture answers through `lib/a.dart` joined onto the package
    /// directory. (The retired resolution canonicalized the raw path
    /// against the cwd and reported `Missing` for any cwd without one.)
    #[test]
    fn relative_path_resolves_against_dir() {
        let root = fixture("relative");
        let args = ImpactArgs {
            path: PathBuf::from("lib/a.dart"),
            dir: root.clone(),
            sdk: Some(root.join("sdk")),
            verbosity: Verbosity::Compact,
            format: crate::OutputFormat::Json,
            symbols: Vec::new(),
        };
        let mut out: Vec<u8> = Vec::new();
        assert_eq!(run(&args, &mut out), exit::CLEAN);
        let json: serde_json::Value =
            serde_json::from_str(&String::from_utf8(out).expect("UTF-8")).expect("one object");
        let symbols: Vec<&str> = json["sym"]
            .as_array()
            .expect("sym")
            .iter()
            .map(|row| row["sym"].as_str().expect("sym"))
            .collect();
        assert_eq!(
            symbols,
            vec!["lib/a.dart|Foo", "lib/a.dart|foo",],
            "the seed is the dir-joined lib/a.dart, its label the located spelling"
        );

        fs::remove_dir_all(&root).ok();
    }

    /// The usage tier covers a target that does not exist at all: the
    /// message names the path as given.
    #[test]
    fn missing_path_is_usage() {
        let root = fixture("missing");
        let args = ImpactArgs {
            path: root.join("lib/nope.dart"),
            dir: root.clone(),
            sdk: Some(root.join("sdk")),
            verbosity: Verbosity::Compact,
            format: crate::OutputFormat::Json,
            symbols: Vec::new(),
        };
        assert_eq!(run(&args, &mut Vec::new()), exit::USAGE);
        fs::remove_dir_all(&root).ok();
    }

    /// The engine assembly failure surfaces as the usage tier through the
    /// shared [`dartq_commands::workspace::WorkspaceError`] message body.
    #[test]
    fn unusable_sdk_is_usage() {
        let root = fixture("sdk");
        let args = ImpactArgs {
            path: root.join("lib/a.dart"),
            dir: root.clone(),
            sdk: Some(root.join("no-such-sdk")),
            verbosity: Verbosity::Compact,
            format: crate::OutputFormat::Json,
            symbols: Vec::new(),
        };
        assert_eq!(run(&args, &mut Vec::new()), exit::USAGE);
        fs::remove_dir_all(&root).ok();
    }
}
