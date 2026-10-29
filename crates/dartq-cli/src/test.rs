//! `dartq test` — `dart test`, faster.
//!
//! The command takes `dart test`'s own arguments and prints what `dart test`
//! prints: the same reporters (`compact`, `expanded`, `failures-only`,
//! `github`, `json`, `silent`), the same `--file-reporter`, the same exit
//! codes (0, 1, 64, 65, 79), the same suite selection (positional files and
//! directories, `dart_test.yaml` `paths:`/`filename:`, the `test/` default,
//! `?name=`/`?line=` selectors). Every other runner option (`-n`, `-N`, `-t`,
//! `-x`, `-p`, `-c`, `-P`, `--timeout`, sharding, …) reaches the runner
//! unchanged. Tests run under `package:test` itself; dartq decides where:
//!
//! - **Result cache.** A suite whose import closure, run-time inputs,
//!   resolution, toolchain, runner arguments and environment hash to the
//!   key of an earlier green run replays that run's reporter events instead
//!   of running (see `dartq_test::cache`). The store is per user, so every
//!   worktree of the same content shares it. Failures are never stored.
//! - **Grouped execution.** Every suite is compiled into one program and
//!   started as an isolate of that program's group (see
//!   `dartq_test::group`), in a few memory-leased processes. A suite whose
//!   outcome there is not provably green, or that observes how it was
//!   launched, runs under the stock runner, which then reports it.
//! - **Admission.** Every run leases its memory from one host-wide pool
//!   shared by all dartq processes of the user (see `dartq_test::host`).
//!
//! Anything this command cannot reproduce exactly runs as plain
//! `dart test` with the same arguments and the terminal attached: `--help`,
//! `--version`, malformed arguments (whose error text and usage are the
//! runner's own), `--debug`, `--pause-after-load`, coverage, `--fail-fast`,
//! a randomized test order, `--configuration`, non-VM platforms, and a
//! `dart_test.yaml` that names a reporter. `--flutter` takes `flutter
//! test`'s arguments instead (see `flutter`).
//!
//! dartq's own options: `--no-cache` (run everything, store nothing),
//! `--impacted[=REF]` (run only the suites the changes since REF can reach;
//! default `HEAD`), `--stats` (one line on stderr saying where suites ran).

use std::borrow::Cow;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use clap::Args;
use rayon::prelude::*;
use rustc_hash::{FxHashMap, FxHashSet};

use dartq_test::event::Json;
use dartq_test::report::{self, Kind};
use dartq_test::{ExecConfig, Settled, SuiteBlock};

use crate::exit;

mod flutter;

/// `package:test`'s exit codes (`exit_codes.dart`).
const NO_TESTS: u8 = 79;

/// Longest a runner may go without writing a line before it is killed,
/// unless `DARTQ_TEST_IDLE_TIMEOUT` (seconds) says otherwise. A runner that
/// keeps reporting is never cut short: each test's own timeout is the
/// runner's to enforce.
const IDLE_TIMEOUT: Duration = Duration::from_secs(30 * 60);

#[derive(Args)]
#[command(disable_help_flag = true)]
pub(crate) struct TestArgs {
    /// `dart test` arguments, plus `--no-cache`, `--impacted[=REF]` and
    /// `--stats`.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 0..)]
    pub(crate) args: Vec<String>,
}

// ---------------------------------------------------------------------------
// Grammar
// ---------------------------------------------------------------------------

/// How an option of the runner's grammar takes values.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// A flag; `true` when `--no-<name>` negates it.
    Flag(bool),
    Single,
    Multi,
}

/// What dartq does with an option.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    /// Passed to every runner unchanged.
    Pass,
    /// Only plain `dart test` reproduces it.
    Delegate,
    Reporter,
    FileReporter,
    Concurrency,
    Color,
    Name,
    PlainName,
    Include,
    Exclude,
    Platform,
    ChainStackTraces,
    /// Delegated when set (`--fail-fast`), passed when negated.
    DelegateWhenOn,
    ImpactedExtra,
    NoCacheExtra,
    StatsExtra,
    FlutterExtra,
    /// Passed, and the run is never stored nor replayed (it writes files
    /// or reads inputs the key does not cover).
    PassUncached,
    /// `flutter test --[no-]pub`: dartq resolves once, children never do.
    Pub,
    /// `flutter test --machine`: the JSON reporter plus the tool's own
    /// device events.
    Machine,
    /// `flutter test --platform`: `tester` runs here, anything else stock.
    FlutterPlatform,
    /// `flutter test -j`: `int.tryParse`, an unparseable value is ignored.
    FlutterConcurrency,
}

/// `package:test`'s argument grammar (`runner/configuration/args.dart`),
/// plus dartq's own options.
const OPTIONS: &[(&str, Option<char>, Shape, Role)] = &[
    ("help", Some('h'), Shape::Flag(false), Role::Delegate),
    ("version", None, Shape::Flag(false), Role::Delegate),
    ("name", Some('n'), Shape::Multi, Role::Name),
    ("plain-name", Some('N'), Shape::Multi, Role::PlainName),
    ("tags", Some('t'), Shape::Multi, Role::Include),
    ("tag", None, Shape::Multi, Role::Include),
    ("exclude-tags", Some('x'), Shape::Multi, Role::Exclude),
    ("exclude-tag", None, Shape::Multi, Role::Exclude),
    ("run-skipped", None, Shape::Flag(true), Role::Pass),
    ("platform", Some('p'), Shape::Multi, Role::Platform),
    ("compiler", Some('c'), Shape::Multi, Role::Pass),
    ("preset", Some('P'), Shape::Multi, Role::Pass),
    ("concurrency", Some('j'), Shape::Single, Role::Concurrency),
    ("total-shards", None, Shape::Single, Role::Pass),
    ("shard-index", None, Shape::Single, Role::Pass),
    ("pub-serve", None, Shape::Single, Role::Delegate),
    ("timeout", None, Shape::Single, Role::Pass),
    ("suite-load-timeout", None, Shape::Single, Role::Pass),
    ("ignore-timeouts", None, Shape::Flag(false), Role::Pass),
    ("pause-after-load", None, Shape::Flag(false), Role::Delegate),
    ("debug", None, Shape::Flag(false), Role::Delegate),
    ("coverage", None, Shape::Single, Role::Delegate),
    ("coverage-path", None, Shape::Single, Role::Delegate),
    ("branch-coverage", None, Shape::Flag(false), Role::Delegate),
    ("coverage-package", None, Shape::Multi, Role::Delegate),
    (
        "chain-stack-traces",
        None,
        Shape::Flag(true),
        Role::ChainStackTraces,
    ),
    ("no-retry", None, Shape::Flag(false), Role::Pass),
    (
        "use-data-isolate-strategy",
        None,
        Shape::Flag(false),
        Role::Pass,
    ),
    (
        "test-randomize-ordering-seed",
        None,
        Shape::Single,
        Role::Delegate,
    ),
    ("fail-fast", None, Shape::Flag(true), Role::DelegateWhenOn),
    ("reporter", Some('r'), Shape::Single, Role::Reporter),
    ("file-reporter", None, Shape::Single, Role::FileReporter),
    ("verbose-trace", None, Shape::Flag(false), Role::Pass),
    ("js-trace", None, Shape::Flag(false), Role::Pass),
    ("color", None, Shape::Flag(true), Role::Color),
    ("configuration", None, Shape::Single, Role::Delegate),
    ("dart2js-args", None, Shape::Multi, Role::Pass),
    ("precompiled", None, Shape::Single, Role::Delegate),
    // dartq's own.
    ("impacted", None, Shape::Flag(false), Role::ImpactedExtra),
    ("no-cache", None, Shape::Flag(false), Role::NoCacheExtra),
    ("stats", None, Shape::Flag(false), Role::StatsExtra),
    ("flutter", None, Shape::Flag(false), Role::FlutterExtra),
];

/// `flutter test`'s argument grammar (`commands/test.dart` plus the
/// command-runner globals it accepts), plus dartq's own options. Options
/// whose effect only the tool reproduces (debugging, coverage, devices,
/// web, integration tests, verbose logging) delegate.
const FLUTTER_OPTIONS: &[(&str, Option<char>, Shape, Role)] = &[
    ("help", Some('h'), Shape::Flag(false), Role::Delegate),
    ("verbose", Some('v'), Shape::Flag(false), Role::Delegate),
    ("version", None, Shape::Flag(false), Role::Delegate),
    ("name", None, Shape::Multi, Role::Name),
    ("plain-name", None, Shape::Multi, Role::PlainName),
    ("tags", Some('t'), Shape::Single, Role::Include),
    ("exclude-tags", Some('x'), Shape::Single, Role::Exclude),
    ("start-paused", None, Shape::Flag(false), Role::Delegate),
    ("fail-fast", None, Shape::Flag(true), Role::DelegateWhenOn),
    ("run-skipped", None, Shape::Flag(true), Role::Pass),
    (
        "disable-service-auth-codes",
        None,
        Shape::Flag(false),
        Role::Delegate,
    ),
    (
        "disable-service-origin-check",
        None,
        Shape::Flag(false),
        Role::Delegate,
    ),
    ("coverage", None, Shape::Flag(false), Role::Delegate),
    ("merge-coverage", None, Shape::Flag(false), Role::Delegate),
    ("branch-coverage", None, Shape::Flag(false), Role::Delegate),
    ("ipv6", None, Shape::Flag(false), Role::Delegate),
    ("coverage-path", None, Shape::Single, Role::Delegate),
    ("coverage-package", None, Shape::Multi, Role::Delegate),
    (
        "update-goldens",
        None,
        Shape::Flag(false),
        Role::PassUncached,
    ),
    (
        "concurrency",
        Some('j'),
        Shape::Single,
        Role::FlutterConcurrency,
    ),
    ("test-assets", None, Shape::Flag(true), Role::Pass),
    ("platform", None, Shape::Single, Role::FlutterPlatform),
    (
        "test-randomize-ordering-seed",
        None,
        Shape::Single,
        Role::Delegate,
    ),
    ("total-shards", None, Shape::Single, Role::Pass),
    ("shard-index", None, Shape::Single, Role::Pass),
    ("enable-vmservice", None, Shape::Flag(true), Role::Delegate),
    ("reporter", Some('r'), Shape::Single, Role::Reporter),
    ("file-reporter", None, Shape::Single, Role::FileReporter),
    ("timeout", None, Shape::Single, Role::Pass),
    ("ignore-timeouts", None, Shape::Flag(false), Role::Pass),
    ("wasm", None, Shape::Flag(false), Role::Delegate),
    (
        "cross-origin-isolation",
        None,
        Shape::Flag(true),
        Role::Delegate,
    ),
    ("uninstall", None, Shape::Flag(true), Role::Delegate),
    (
        "experimental-faster-testing",
        None,
        Shape::Flag(false),
        Role::Delegate,
    ),
    ("machine", None, Shape::Flag(false), Role::Machine),
    ("pub", None, Shape::Flag(true), Role::Pub),
    (
        "frontend-server-starter-path",
        None,
        Shape::Single,
        Role::Delegate,
    ),
    ("track-widget-creation", None, Shape::Flag(true), Role::Pass),
    ("enable-experiment", None, Shape::Multi, Role::Pass),
    ("dart-define", Some('D'), Shape::Multi, Role::Pass),
    (
        "dart-define-from-file",
        None,
        Shape::Multi,
        Role::PassUncached,
    ),
    ("device-user", None, Shape::Single, Role::Delegate),
    ("flavor", None, Shape::Single, Role::Pass),
    ("enable-impeller", None, Shape::Flag(true), Role::Pass),
    ("enable-flutter-gpu", None, Shape::Flag(true), Role::Pass),
    ("enable-hcpp", None, Shape::Flag(true), Role::Pass),
    ("dds-port", None, Shape::Single, Role::Delegate),
    ("dds", None, Shape::Flag(true), Role::Delegate),
    ("disable-dds", None, Shape::Flag(true), Role::Delegate),
    ("fatal-warnings", None, Shape::Flag(true), Role::Delegate),
    // Command-runner globals.
    ("color", None, Shape::Flag(true), Role::Color),
    ("suppress-analytics", None, Shape::Flag(false), Role::Pass),
    ("version-check", None, Shape::Flag(true), Role::Pass),
    ("device-id", Some('d'), Shape::Single, Role::Delegate),
    ("packages", None, Shape::Single, Role::Delegate),
    // dartq's own.
    ("impacted", None, Shape::Flag(false), Role::ImpactedExtra),
    ("no-cache", None, Shape::Flag(false), Role::NoCacheExtra),
    ("stats", None, Shape::Flag(false), Role::StatsExtra),
    ("flutter", None, Shape::Flag(false), Role::FlutterExtra),
];

/// Whether `args` asks for `flutter test` (`--flutter` before any `--`).
fn wants_flutter(args: &[String]) -> bool {
    args.iter()
        .take_while(|arg| *arg != "--")
        .any(|arg| arg == "--flutter")
}

/// A parsed command line.
#[derive(Debug, Default, PartialEq)]
struct Invocation {
    /// Something only plain `dart test` reproduces was asked for.
    delegate: bool,
    flutter: bool,
    /// Positional selections, as typed.
    paths: Vec<String>,
    /// Options every runner gets, in `--name=value` form, in order.
    pass: Vec<String>,
    reporter: Option<Kind>,
    /// `--file-reporter <reporter>:<path>`.
    file_reporter: Option<(Kind, PathBuf)>,
    concurrency: Option<usize>,
    color: Option<bool>,
    names: Vec<String>,
    plain_names: Vec<String>,
    include_tags: Vec<String>,
    exclude_tags: Vec<String>,
    chain_stack_traces: bool,
    /// `--impacted[=REF]`.
    impacted: Option<String>,
    no_cache: bool,
    stats: bool,
    /// `flutter test --machine`.
    machine: bool,
    /// `flutter test --no-pub`.
    no_pub: bool,
}

/// Parse `args` the way `package:args` parses the runner's grammar
/// (options anywhere, `--` ends them, `--name=value` or `--name value`,
/// `-x value`/`-xvalue`, `--no-` negation). `Err` when the line is not one
/// the runner would accept: the caller hands it to `dart test`, which
/// reports the error in its own words.
fn parse(args: &[String]) -> Result<Invocation, ()> {
    let table = if wants_flutter(args) {
        FLUTTER_OPTIONS
    } else {
        OPTIONS
    };
    let mut inv = Invocation::default();
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        index += 1;
        if arg == "--" {
            inv.paths.extend(args[index..].iter().cloned());
            break;
        }
        let (spec, value) = if let Some(long) = arg.strip_prefix("--") {
            let (name, inline) = match long.split_once('=') {
                Some((name, value)) => (name, Some(value.to_owned())),
                None => (long, None),
            };
            if name == "impacted" {
                inv.impacted = Some(inline.unwrap_or_else(|| "HEAD".to_owned()));
                continue;
            }
            match table.iter().find(|option| option.0 == name) {
                Some(option) => (option, inline.map(Some).unwrap_or(None)),
                None => {
                    let negated = name.strip_prefix("no-").and_then(|base| {
                        table
                            .iter()
                            .find(|option| option.0 == base && option.2 == Shape::Flag(true))
                    });
                    let Some(option) = negated else {
                        return Err(());
                    };
                    if inline.is_some() {
                        return Err(());
                    }
                    apply(&mut inv, option, None, false)?;
                    continue;
                }
            }
        } else if let Some(short) = arg.strip_prefix('-').filter(|rest| !rest.is_empty()) {
            let mut chars = short.chars();
            let first = chars.next().expect("non-empty");
            let option = table
                .iter()
                .find(|option| option.1 == Some(first))
                .ok_or(())?;
            let rest: String = chars.collect();
            match option.2 {
                Shape::Flag(_) => {
                    // `-hv`-style flag clusters: every letter a flag.
                    apply(&mut inv, option, None, true)?;
                    for c in rest.chars() {
                        let flag = table.iter().find(|option| option.1 == Some(c)).ok_or(())?;
                        if !matches!(flag.2, Shape::Flag(_)) {
                            return Err(());
                        }
                        apply(&mut inv, flag, None, true)?;
                    }
                    continue;
                }
                _ => (option, (!rest.is_empty()).then_some(rest)),
            }
        } else {
            inv.paths.push(arg.clone());
            continue;
        };
        match spec.2 {
            Shape::Flag(_) => {
                if value.is_some() {
                    return Err(());
                }
                apply(&mut inv, spec, None, true)?;
            }
            Shape::Single | Shape::Multi => {
                let value = match value {
                    Some(value) => value,
                    None => {
                        let next = args.get(index).ok_or(())?.clone();
                        index += 1;
                        next
                    }
                };
                apply(&mut inv, spec, Some(value), true)?;
            }
        }
    }
    Ok(inv)
}

/// Record one parsed option.
fn apply(
    inv: &mut Invocation,
    option: &(&str, Option<char>, Shape, Role),
    value: Option<String>,
    on: bool,
) -> Result<(), ()> {
    let (name, _, shape, role) = *option;
    let token = match (&value, shape) {
        (Some(value), _) => format!("--{name}={value}"),
        (None, _) if on => format!("--{name}"),
        (None, _) => format!("--no-{name}"),
    };
    match role {
        Role::Pass => inv.pass.push(token),
        Role::Delegate => inv.delegate = true,
        Role::DelegateWhenOn => {
            if on {
                inv.delegate = true;
            } else {
                inv.pass.push(token);
            }
        }
        Role::Reporter => inv.reporter = Some(Kind::parse(value.as_deref().ok_or(())?).ok_or(())?),
        Role::FileReporter => {
            let value = value.ok_or(())?;
            let (kind, path) = value.split_once(':').ok_or(())?;
            inv.file_reporter = Some((Kind::parse(kind).ok_or(())?, PathBuf::from(path)));
        }
        Role::Concurrency => {
            // `int.parse`: an optional sign and decimal digits. Anything
            // below one is the runner's own error to report.
            let value: i64 = value.ok_or(())?.parse().map_err(|_| ())?;
            match usize::try_from(value) {
                Ok(value) if value >= 1 => inv.concurrency = Some(value),
                _ => inv.delegate = true,
            }
        }
        Role::Color => inv.color = Some(on),
        Role::Name => {
            let value = value.ok_or(())?;
            inv.names.push(value);
            inv.pass.push(token);
        }
        Role::PlainName => {
            let value = value.ok_or(())?;
            inv.plain_names.push(value);
            inv.pass.push(token);
        }
        Role::Include => {
            inv.include_tags.push(value.ok_or(())?);
            inv.pass.push(token);
        }
        Role::Exclude => {
            inv.exclude_tags.push(value.ok_or(())?);
            inv.pass.push(token);
        }
        Role::Platform => {
            let value = value.ok_or(())?;
            if value.split(',').any(|platform| platform.trim() != "vm") {
                inv.delegate = true;
            }
            inv.pass.push(token);
        }
        Role::ChainStackTraces => {
            inv.chain_stack_traces = on;
            inv.pass.push(token);
        }
        Role::ImpactedExtra => inv.impacted = Some("HEAD".to_owned()),
        Role::NoCacheExtra => inv.no_cache = true,
        Role::StatsExtra => inv.stats = true,
        Role::FlutterExtra => inv.flutter = true,
        Role::PassUncached => {
            inv.no_cache = true;
            inv.pass.push(token);
        }
        Role::Pub => inv.no_pub = !on,
        Role::Machine => inv.machine = true,
        Role::FlutterPlatform => {
            if value.as_deref() != Some("tester") {
                inv.delegate = true;
            }
        }
        Role::FlutterConcurrency => match value.ok_or(())?.parse::<i64>() {
            Ok(jobs) if jobs >= 1 => inv.concurrency = usize::try_from(jobs).ok(),
            // The tool's own error.
            Ok(_) => inv.delegate = true,
            // `int.tryParse` failing leaves the default.
            Err(_) => {}
        },
    }
    Ok(())
}

/// The command line without dartq's own options, for plain `dart test`.
fn runner_args(args: &[String]) -> Vec<String> {
    let mut out = Vec::with_capacity(args.len());
    let mut positional = false;
    for arg in args {
        if !positional {
            if arg == "--" {
                positional = true;
            } else if matches!(
                arg.as_str(),
                "--no-cache" | "--stats" | "--flutter" | "--impacted"
            ) || arg.starts_with("--impacted=")
            {
                continue;
            }
        }
        out.push(arg.clone());
    }
    out
}

// ---------------------------------------------------------------------------
// Delegation
// ---------------------------------------------------------------------------

/// The one line a delegation prints: where the run went, and why.
fn delegation_line(flutter: bool, reason: &str) -> String {
    format!(
        "dartq: delegating to {} — {reason}",
        if flutter { "flutter test" } else { "dart test" }
    )
}

/// `delegate`, after naming the reason on stderr: the line a delegation
/// the repository's state forced always prints, so a quiet hand-off is
/// never a mystery.
fn delegate_with(args: &[String], flutter: bool, reason: &str) -> u8 {
    eprintln!("{}", delegation_line(flutter, reason));
    delegate(args, flutter)
}

/// A delegation the invocation itself asked for (`--help`, coverage, a
/// platform only the runner hosts): the line prints only when `--stats`
/// asked where suites ran.
fn delegate_requested(args: &[String], flutter: bool, stats: bool, reason: &str) -> u8 {
    if stats {
        eprintln!("{}", delegation_line(flutter, reason));
    }
    delegate(args, flutter)
}

/// Why the repository's state sends the run to the stock runner, when it
/// does: preconditions only a resolved environment gives back, named in
/// the gate's own order. `global_config` is `global_config_exists()`,
/// passed in so the mapping stays pure.
fn state_reason(root: &Path, global_config: bool) -> Option<Cow<'static, str>> {
    if !root.join("pubspec.yaml").is_file() {
        return Some("no pubspec.yaml here".into());
    }
    let config = dartq_test::cache::package_config_path(root);
    if !config.is_file() {
        return Some(format!("no resolved {} — run dart pub get", config.display()).into());
    }
    global_config.then(|| "global dart_test config present".into())
}

/// Run plain `dart test` (or `flutter test`) with `args` and the terminal
/// attached, returning its exit status.
fn delegate(args: &[String], flutter: bool) -> u8 {
    let bin = if flutter {
        dartq_test::host::find("flutter").unwrap_or_else(|| PathBuf::from("flutter"))
    } else {
        dartq_test::host::find("dart").unwrap_or_else(|| PathBuf::from("dart"))
    };
    match std::process::Command::new(&bin)
        .arg("test")
        .args(runner_args(args))
        .status()
    {
        Ok(status) => status
            .code()
            .map_or(exit::INTERNAL_ERROR, |code| u8::try_from(code).unwrap_or(1)),
        Err(error) => {
            eprintln!("dartq: cannot run {}: {error}", bin.display());
            exit::INTERNAL_ERROR
        }
    }
}

// ---------------------------------------------------------------------------
// Selection
// ---------------------------------------------------------------------------

/// One suite: the path the runner reports it under, and the file.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Suite {
    spelling: String,
    abs: PathBuf,
}

/// What `dart_test.yaml` contributes to selection and scheduling.
#[derive(Debug, Default)]
pub(super) struct FileConfig {
    pub(super) paths: Option<Vec<String>>,
    pub(super) filename: Option<String>,
    pub(super) concurrency: Option<usize>,
    /// The file names suite selection of its own (tag selectors, platform
    /// selectors, per-tag configuration): only the runner knows which
    /// suites it loads.
    pub(super) selects: bool,
}

/// Keys of `dart_test.yaml` that take part in suite selection.
const SELECTION_KEYS: &[&str] = &[
    "include_tags",
    "exclude_tags",
    "add_tags",
    "test_on",
    "on_os",
    "on_platform",
];

/// Keys of `dart_test.yaml` whose effect only the runner reproduces (a
/// reporter, presets, platform definitions, includes): their presence
/// delegates the run.
const RUNNER_ONLY_KEYS: &[&str] = &[
    "reporter",
    "file_reporters",
    "include",
    "presets",
    "add_presets",
    "define_platforms",
    "override_platforms",
    "platforms",
    "pause_after_load",
];

/// Read `dart_test.yaml`; `Err` when it holds something only the runner
/// reproduces (or that it would reject).
pub(super) fn file_config(root: &Path) -> Result<FileConfig, ()> {
    let Ok(text) = std::fs::read_to_string(root.join("dart_test.yaml")) else {
        return Ok(FileConfig::default());
    };
    if text.trim().is_empty() {
        return Ok(FileConfig::default());
    }
    let value: serde_yaml::Value = serde_yaml::from_str(&text).map_err(|_| ())?;
    let Some(map) = value.as_mapping() else {
        return if value.is_null() {
            Ok(FileConfig::default())
        } else {
            Err(())
        };
    };
    let key = |name: &str| map.get(serde_yaml::Value::String(name.to_owned()));
    if RUNNER_ONLY_KEYS.iter().any(|name| key(name).is_some()) {
        return Err(());
    }
    let paths = match key("paths") {
        None => None,
        Some(serde_yaml::Value::Sequence(items)) => Some(
            items
                .iter()
                .map(|item| item.as_str().map(str::to_owned).ok_or(()))
                .collect::<Result<Vec<_>, _>>()?,
        ),
        Some(_) => return Err(()),
    };
    let filename = match key("filename") {
        None => None,
        Some(value) => Some(value.as_str().ok_or(())?.to_owned()),
    };
    let concurrency = match key("concurrency") {
        None => None,
        Some(value) => Some(
            usize::try_from(value.as_i64().ok_or(())?)
                .ok()
                .filter(|&n| n >= 1)
                .ok_or(())?,
        ),
    };
    // A `tags:` map only declares tags unless some tag carries settings.
    let tag_settings = match key("tags") {
        None => false,
        Some(serde_yaml::Value::Mapping(tags)) => tags.values().any(|value| !value.is_null()),
        Some(_) => true,
    };
    let selects = tag_settings || SELECTION_KEYS.iter().any(|name| key(name).is_some());
    Ok(FileConfig {
        paths,
        filename,
        concurrency,
        selects,
    })
}

/// The user configuration file `package:test` merges under every run
/// (`DART_TEST_CONFIG`, else `~/.dart_test.yaml`).
fn global_config_exists() -> bool {
    let path = std::env::var_os("DART_TEST_CONFIG")
        .map(PathBuf::from)
        .or_else(|| {
            if cfg!(windows) {
                std::env::var_os("LOCALAPPDATA").map(|dir| PathBuf::from(dir).join("DartTest.yaml"))
            } else {
                std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".dart_test.yaml"))
            }
        });
    path.is_some_and(|path| path.exists())
}

/// A selection's path as the runner keys it: the path of the selection
/// read as a URI reference, dot segments removed (`./test` → `test`,
/// `test/a/../b` → `test/b`; a leading `..` stays). `None` for a spelling
/// that carries URI syntax the runner would interpret (a query, a fragment,
/// a percent escape): the caller delegates those.
fn spell(selection: &str) -> Option<String> {
    if selection.contains(['?', '#', '%']) {
        return None;
    }
    let absolute = selection.starts_with('/');
    let mut out: Vec<&str> = Vec::new();
    let segments: Vec<&str> = selection.split('/').collect();
    let mut trailing = false;
    for (index, segment) in segments.iter().enumerate() {
        let last = index + 1 == segments.len();
        match *segment {
            "." => trailing = last,
            ".." => {
                if out.last().is_some_and(|prev| *prev != "..") && !(absolute && out.len() == 1) {
                    out.pop();
                } else if !absolute {
                    out.push("..");
                }
                trailing = last;
            }
            "" if index == 0 && absolute => out.push(""),
            segment => {
                out.push(segment);
                trailing = false;
            }
        }
    }
    let mut spelled = out.join("/");
    if trailing && !spelled.is_empty() {
        spelled.push('/');
    }
    (!spelled.is_empty()).then_some(spelled)
}

/// Every suite the selections reach, the runner's way and in its order:
/// each selection is a file or a directory; a directory contributes every
/// file beneath it (hidden and `build` directories included, links
/// followed) whose name matches `filename`, in the directory listing's own
/// order, entries before the subdirectories they appear among, exactly as
/// `listSync(recursive: true)` yields them. `None` when the runner would report something this
/// command does not reproduce (a missing selection, a selection reached
/// twice, a selector query): the caller delegates.
fn discover(root: &Path, selections: &[String], filename: Option<&str>) -> Option<Vec<Suite>> {
    let glob = match filename {
        Some(pattern) => Some(dartq_pub::glob::Glob::new(pattern).ok()?),
        None => None,
    };
    let matches = |name: &str| {
        glob.as_ref().map_or_else(
            || name.ends_with("_test.dart"),
            |glob| glob.matches_path(name),
        )
    };
    let mut suites: Vec<Suite> = Vec::new();
    let mut keys: FxHashSet<String> = FxHashSet::default();
    for selection in selections {
        let spelled = spell(selection)?;
        if !keys.insert(spelled.clone()) {
            continue;
        }
        let full = if Path::new(&spelled).is_absolute() {
            PathBuf::from(&spelled)
        } else {
            root.join(&spelled)
        };
        let meta = std::fs::metadata(&full).ok()?;
        if meta.is_dir() {
            let base = spelled.trim_end_matches('/');
            let mut found = Vec::new();
            let mut seen = FxHashSet::default();
            walk(&full, "", &mut found, &mut seen).ok()?;
            for rel in found {
                let name = rel.rsplit('/').next().unwrap_or(&rel);
                if matches(name) {
                    let spelling = if base.is_empty() {
                        rel.clone()
                    } else {
                        format!("{base}/{rel}")
                    };
                    suites.push(Suite {
                        abs: full.join(&rel).canonicalize().ok()?,
                        spelling,
                    });
                }
            }
        } else {
            suites.push(Suite {
                abs: full.canonicalize().ok()?,
                spelling: spelled,
            });
        }
    }
    let mut files: FxHashSet<&Path> = FxHashSet::default();
    if !suites.iter().all(|suite| files.insert(&suite.abs)) {
        // The runner loads a file once per selection reaching it.
        return None;
    }
    Some(suites)
}

/// Files beneath `dir` as `/`-joined paths relative to it. Directory
/// links are followed once per canonical target, so a cycle cannot loop.
fn walk(
    dir: &Path,
    prefix: &str,
    out: &mut Vec<String>,
    seen: &mut FxHashSet<PathBuf>,
) -> io::Result<()> {
    if !seen.insert(dir.canonicalize()?) {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let rel = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        let Ok(meta) = std::fs::metadata(entry.path()) else {
            continue; // a dangling link
        };
        if meta.is_dir() {
            walk(&entry.path(), &rel, out, seen)?;
        } else if meta.is_file() {
            out.push(rel);
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Verdicts
// ---------------------------------------------------------------------------

/// What one suite's events prove.
#[derive(Debug, Default, Clone, Copy)]
struct Tally {
    passed: usize,
    skipped: usize,
    failed: usize,
    /// No error and no non-success outcome anywhere, the hidden load test
    /// included.
    green: bool,
    /// A test started and never finished (a suite that called `exit()`
    /// mid-test): its outcome is unknown, so the run cannot be green.
    incomplete: bool,
}

/// Tally a suite's events the way the runner counts its engine: hidden
/// tests (suite loads, `setUpAll`, `tearDownAll`) count only through their
/// errors, which make the run fail.
fn tally(events: &[Json]) -> Tally {
    let mut tally = Tally {
        green: true,
        ..Tally::default()
    };
    // An error after a test finished still fails it (the runner moves it
    // from passed to failed), so errors are gathered first.
    let errored: FxHashSet<u64> = events
        .iter()
        .filter(|event| event.kind() == "error")
        .filter_map(|event| event.u64_at(&["testID"]))
        .collect();
    let mut open: FxHashSet<u64> = FxHashSet::default();
    for event in events {
        match event.kind() {
            "error" => tally.green = false,
            "testStart" => {
                if let Some(id) = event.u64_at(&["test", "id"]) {
                    open.insert(id);
                }
            }
            "testDone" => {
                if let Some(id) = event.u64_at(&["testID"]) {
                    open.remove(&id);
                }
                let hidden = event.get("hidden").and_then(Json::as_bool).unwrap_or(false);
                let skipped = event
                    .get("skipped")
                    .and_then(Json::as_bool)
                    .unwrap_or(false);
                let success = event.str_at(&["result"]) == Some("success");
                let failed = !success
                    || event
                        .u64_at(&["testID"])
                        .is_some_and(|id| errored.contains(&id));
                if !success {
                    tally.green = false;
                }
                if hidden {
                    if failed {
                        tally.failed += 1;
                    }
                } else if skipped && !failed {
                    tally.skipped += 1;
                } else if failed {
                    tally.failed += 1;
                } else {
                    tally.passed += 1;
                }
            }
            _ => {}
        }
    }
    tally.incomplete = !open.is_empty();
    tally
}

/// A suite block is a complete record of its suite: it was closed, and
/// every test it started also finished.
fn complete(block: &SuiteBlock) -> bool {
    if !block.closed {
        return false;
    }
    let mut open: FxHashSet<u64> = FxHashSet::default();
    for event in &block.events {
        match event.kind() {
            "testStart" => {
                if let Some(id) = event.u64_at(&["test", "id"]) {
                    open.insert(id);
                }
            }
            "testDone" => {
                if let Some(id) = event.u64_at(&["testID"]) {
                    open.remove(&id);
                }
            }
            _ => {}
        }
    }
    open.is_empty()
}

// ---------------------------------------------------------------------------
// Output
// ---------------------------------------------------------------------------

/// Every reporter the run writes to: the main one on stdout, plus the
/// `--file-reporter` one.
struct Outputs {
    main: Sink,
    file: Option<Sink>,
}

enum Sink {
    Human(report::Renderer, Box<dyn Write>),
    Compact(report::Compact, Box<dyn Write>),
    Github(report::Github, Box<dyn Write>),
    Json(report::JsonMerge, Box<dyn Write>),
    /// `silent` on stdout: test output still prints.
    SilentPrints(Box<dyn Write>),
    /// `silent` as a file reporter: nothing at all.
    Silent,
}

impl Sink {
    fn new(
        kind: Kind,
        opts: &report::Options,
        mut out: Box<dyn Write>,
        version: Option<&str>,
        started: Instant,
        total: usize,
    ) -> io::Result<Self> {
        Ok(match kind {
            Kind::Json => Self::Json(
                report::JsonMerge::start(out.as_mut(), version, started, total as u64)?,
                out,
            ),
            Kind::Github => {
                let mut github = report::Github::new(opts.print_path);
                github.start(out.as_mut())?;
                Self::Github(github, out)
            }
            Kind::Silent => Self::SilentPrints(out),
            Kind::Compact => Self::Compact(report::Compact::new(opts.clone()), out),
            kind => Self::Human(report::Renderer::new(kind, opts.clone()), out),
        })
    }

    /// A file reporter: only the reporter writes to it (test output goes to
    /// stdout through the main reporter).
    fn file(
        kind: Kind,
        opts: &report::Options,
        out: Box<dyn Write>,
        version: Option<&str>,
        started: Instant,
        total: usize,
    ) -> io::Result<Self> {
        if kind == Kind::Silent {
            return Ok(Self::Silent);
        }
        Self::new(kind, opts, out, version, started, total)
    }

    fn block(&mut self, events: &[Json]) -> io::Result<()> {
        match self {
            // No reporter listens for test output, so the runner prints
            // every message as it arrives.
            Self::SilentPrints(out) => {
                for event in events.iter().filter(|event| {
                    event.kind() == "print" || event.kind() == dartq_test::event::SHELL
                }) {
                    writeln!(out, "{}", event.str_at(&["message"]).unwrap_or(""))?;
                }
                out.flush()
            }
            Self::Human(renderer, out) => {
                renderer.block(events, out.as_mut())?;
                out.flush()
            }
            Self::Compact(compact, out) => {
                compact.block(events, out.as_mut())?;
                out.flush()
            }
            Self::Github(github, out) => {
                github.block(events, out.as_mut())?;
                out.flush()
            }
            Self::Json(merge, out) => {
                merge.block(events, out.as_mut())?;
                out.flush()
            }
            Self::Silent => Ok(()),
        }
    }

    /// The GitHub reporter of the engine `flutter test` pins.
    fn legacy_github(&mut self) {
        if let Self::Github(github, _) = self {
            github.legacy = true;
        }
    }

    /// `flutter test --machine`: the tool's device event after each
    /// suite's load starts.
    fn machine(&mut self) {
        if let Self::Json(merge, _) = self {
            merge.machine = true;
        }
    }

    fn loose(&mut self, event: &Json) -> io::Result<()> {
        match self {
            Self::Json(merge, out) => merge.loose(event, out.as_mut()),
            _ => Ok(()),
        }
    }

    fn done(&mut self, success: bool) -> io::Result<()> {
        match self {
            Self::Human(renderer, out) => {
                renderer.done(out.as_mut())?;
                out.flush()
            }
            Self::Compact(compact, out) => {
                compact.done(out.as_mut())?;
                out.flush()
            }
            Self::Github(github, out) => {
                github.done(out.as_mut())?;
                out.flush()
            }
            Self::Json(merge, out) => {
                merge.done(success, out.as_mut())?;
                out.flush()
            }
            Self::SilentPrints(out) => out.flush(),
            Self::Silent => Ok(()),
        }
    }
}

impl Outputs {
    fn each(&mut self, mut f: impl FnMut(&mut Sink) -> io::Result<()>) -> io::Result<()> {
        f(&mut self.main)?;
        if let Some(file) = self.file.as_mut() {
            f(file)?;
        }
        Ok(())
    }

    /// One suite's events: the main reporter renders them as-is; a file
    /// reporter without the runner's own raw lines (the tool wrote those to
    /// standard output only, no reporter ever saw them).
    fn block(&mut self, events: &[Json]) -> io::Result<()> {
        self.main.block(events)?;
        let Some(file) = self.file.as_mut() else {
            return Ok(());
        };
        if events
            .iter()
            .any(|event| event.kind() == dartq_test::event::SHELL)
        {
            let stripped: Vec<Json> = events
                .iter()
                .filter(|event| event.kind() != dartq_test::event::SHELL)
                .cloned()
                .collect();
            file.block(&stripped)
        } else {
            file.block(events)
        }
    }
}

/// The `test` package version the runner reports in its `start` event
/// (`pubspec.lock`'s `test` entry).
fn runner_version(root: &Path) -> Option<String> {
    let text = std::fs::read_to_string(root.join("pubspec.lock")).ok()?;
    let lock = dartq_pub::LockFile::parse(&text).ok()?;
    Some(lock.package("test")?.version.to_string())
}

// ---------------------------------------------------------------------------
// The run
// ---------------------------------------------------------------------------

pub(crate) fn run_entry(args: &TestArgs) -> u8 {
    let outcome = crate::crash_tier(|| run(&args.args));
    outcome.unwrap_or_else(|_| {
        eprintln!("dartq: internal error: the test face panicked");
        exit::INTERNAL_ERROR
    })
}

/// Everything one run needs, resolved up front.
struct Plan {
    root: PathBuf,
    dart: PathBuf,
    inv: Invocation,
    /// Every suite to report, in the order its result is emitted.
    suites: Vec<Suite>,
    workers: usize,
    print_path: bool,
}

fn run(args: &[String]) -> u8 {
    let Ok(inv) = parse(args) else {
        // No `Invocation`, so `--stats` is read off the line itself.
        let stats = args
            .iter()
            .take_while(|arg| *arg != "--")
            .any(|arg| arg == "--stats");
        return delegate_requested(args, false, stats, "unrecognized arguments");
    };
    if inv.flutter {
        return flutter::run(args, inv);
    }
    if inv.delegate {
        return delegate_requested(args, false, inv.stats, "tool-only options requested");
    }
    let root = match std::env::current_dir().and_then(|dir| dir.canonicalize()) {
        Ok(root) => root,
        Err(error) => {
            eprintln!("dartq: cannot resolve the current directory: {error}");
            return exit::INTERNAL_ERROR;
        }
    };
    // The runner's own preconditions and configuration errors are its to
    // report.
    if let Some(reason) = state_reason(&root, global_config_exists()) {
        return delegate_with(args, false, &reason);
    }
    let Ok(config) = file_config(&root) else {
        return delegate_with(args, false, "dart_test.yaml present");
    };
    let explicit = !inv.paths.is_empty();
    let selections: Vec<String> = if explicit {
        inv.paths.clone()
    } else {
        config
            .paths
            .clone()
            .unwrap_or_else(|| vec!["test".to_owned()])
    };
    if !explicit && config.paths.is_none() && !root.join("test").is_dir() {
        return delegate_with(args, false, "no test/ directory here");
    }
    let Some(mut suites) = discover(&root, &selections, config.filename.as_deref()) else {
        return delegate_with(args, false, "no test suites discovered");
    };
    let print_path = selections.len() > 1
        || root
            .join(spell(&selections[0]).unwrap_or_default())
            .is_dir();
    if let Some(base) = &inv.impacted {
        match dartq_commands::impact::impacted_tests(&root, None, base) {
            Ok(reached) => {
                let reached: FxHashSet<&str> = reached.iter().map(String::as_str).collect();
                suites.retain(|suite| {
                    suite.abs.strip_prefix(&root).is_ok_and(|rel| {
                        reached.contains(rel.to_string_lossy().replace('\\', "/").as_str())
                    })
                });
                if suites.is_empty() {
                    eprintln!("dartq test: no suite is reachable from the changes since {base}");
                    return exit::CLEAN;
                }
            }
            // The conservative fallback — the shared selection cannot bound
            // the change, so everything runs.
            Err(reason) => eprintln!("dartq test: {reason} — running everything"),
        }
    }
    // Suite-level selection: drop what the runner would never load, so
    // the run reports (and counts) exactly the suites the runner loads.
    // When a suite's selection is uncertain the runner still filters it
    // (a finished run that never reports a suite filtered it out), but the
    // JSON reporter's suite count is announced before that is known, so a
    // JSON run then belongs to the runner.
    let json =
        inv.reporter == Some(Kind::Json) || matches!(inv.file_reporter, Some((Kind::Json, _)));
    if config.selects {
        if json {
            return delegate_requested(
                args,
                false,
                inv.stats,
                "json reporter with an uncertain selection",
            );
        }
    } else {
        let Some(filter) = dartq_test::select::TagFilter::new(&inv.include_tags, &inv.exclude_tags)
        else {
            return delegate_requested(args, false, inv.stats, "invalid tag selector");
        };
        let files: Vec<PathBuf> = suites.iter().map(|suite| suite.abs.clone()).collect();
        let selections = dartq_test::select::select_all(&files, &filter);
        if json && selections.contains(&dartq_test::select::Selection::Unknown) {
            return delegate_requested(
                args,
                false,
                inv.stats,
                "json reporter with an uncertain selection",
            );
        }
        let mut selections = selections.into_iter();
        suites.retain(|_| selections.next() != Some(dartq_test::select::Selection::Dropped));
    }
    let Some(dart) = dartq_test::host::find("dart") else {
        return delegate_with(args, false, "no dart on PATH");
    };
    let workers = inv.concurrency.or(config.concurrency).unwrap_or_else(|| {
        // The runner's default: half the cores, at least one.
        std::thread::available_parallelism().map_or(1, |cores| (cores.get() / 2).max(1))
    });
    let plan = Plan {
        root,
        dart,
        inv,
        suites,
        workers,
        print_path,
    };
    match execute(&plan) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("dartq: internal error: {error}");
            exit::INTERNAL_ERROR
        }
    }
}

/// How each suite was answered, for `--stats`.
#[derive(Default)]
struct Stats {
    replayed: usize,
    grouped: usize,
    stock: usize,
}

/// The `--stats` refusal histogram: each [`dartq_test::Refusal`] label
/// with its suite count, most frequent first (label order breaking ties),
/// so the same run always prints the same string. Empty when no suite was
/// refused — the caller prints no parenthesis then.
fn refusal_summary(refusals: &[Option<dartq_test::Refusal>]) -> String {
    let mut counts: FxHashMap<&'static str, usize> = FxHashMap::default();
    for refusal in refusals.iter().flatten() {
        *counts.entry(refusal.label()).or_default() += 1;
    }
    let mut reasons: Vec<(&'static str, usize)> = counts.into_iter().collect();
    reasons.sort_unstable_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    reasons
        .iter()
        .map(|(label, count)| format!("{count} {label}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Run the plan: replay what the cache proves, run the rest grouped, rerun
/// whatever a grouped run did not prove green under the stock runner, and
/// emit every suite's events in selection order as soon as every suite
/// before it is settled.
fn execute(plan: &Plan) -> io::Result<u8> {
    let started = Instant::now();
    let root = &plan.root;
    let runner_args: Vec<String> = plan.inv.pass.clone();
    let runner = dartq_test::Runner {
        bin: &plan.dart,
        args: &runner_args,
        flutter: false,
    };

    // Keys: suites the cache can vouch for, and their stored events.
    let mut cache = (!plan.inv.no_cache).then(|| dartq_test::Cache::open(root));
    let abs: Vec<PathBuf> = plan.suites.iter().map(|suite| suite.abs.clone()).collect();
    let keys: Vec<Option<String>> = match cache.as_mut().map(|cache| cache.keys(&abs, runner)) {
        Some(Ok(keys)) => keys,
        Some(Err(error)) => {
            eprintln!("dartq test: result cache unavailable ({error}) — running every suite");
            cache = None;
            vec![None; abs.len()]
        }
        None => vec![None; abs.len()],
    };
    let refusals: Vec<Option<dartq_test::Refusal>> = cache
        .as_ref()
        .map(|cache| cache.refusals().to_vec())
        .unwrap_or_default();

    // The output side.
    let color = plan.inv.color.unwrap_or_else(report::color_default);
    let opts = report::Options {
        color,
        print_path: plan.print_path,
        line_length: terminal_width(),
        executable: vm_executable(&plan.dart).display().to_string(),
        chain_stack_traces: plan.inv.chain_stack_traces,
    };
    let version = runner_version(root);
    let kind = plan
        .inv
        .reporter
        .unwrap_or_else(|| Kind::default_for(color));
    let stdout: Box<dyn Write> = Box::new(BufWriter::new(io::stdout()));
    let mut outputs = Outputs {
        main: Sink::new(
            kind,
            &opts,
            stdout,
            version.as_deref(),
            started,
            plan.suites.len(),
        )?,
        file: match &plan.inv.file_reporter {
            Some((kind, path)) => {
                if let Some(dir) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
                    std::fs::create_dir_all(dir)?;
                }
                let file: Box<dyn Write> = Box::new(BufWriter::new(std::fs::File::create(path)?));
                let file_opts = report::Options {
                    color: false,
                    ..opts.clone()
                };
                Some(Sink::file(
                    *kind,
                    &file_opts,
                    file,
                    version.as_deref(),
                    started,
                    plan.suites.len(),
                )?)
            }
            None => None,
        },
    };

    // Emission state: each suite's final block, emitted in order.
    let mut slots: Vec<Option<Vec<Json>>> = vec![None; plan.suites.len()];
    let mut emitted = 0;
    let mut next_id = 0u64;
    let mut stock_stderr = false;
    let mut clock = 0u64;
    let mut totals = Tally::default();
    let mut stats = Stats::default();
    let mut rerun: Vec<usize> = Vec::new();
    let mut to_run: Vec<usize> = Vec::new();
    // A warm run parses every replayed entry's events: thousands of files,
    // each parsed on its own core.
    let replayed: Vec<Option<Vec<Json>>> = keys
        .par_iter()
        .map(|key| {
            key.as_deref()
                .and_then(|key| cache.as_ref().and_then(|cache| cache.lookup(key)))
        })
        .collect();
    for (index, events) in replayed.into_iter().enumerate() {
        match events {
            Some(events) => {
                slots[index] = Some(events);
                stats.replayed += 1;
            }
            None => to_run.push(index),
        }
    }
    let index_of: FxHashMap<&Path, usize> = plan
        .suites
        .iter()
        .enumerate()
        .map(|(index, suite)| (suite.abs.as_path(), index))
        .collect();
    let mut emit = |slots: &mut Vec<Option<Vec<Json>>>,
                    outputs: &mut Outputs,
                    totals: &mut Tally|
     -> io::Result<()> {
        while emitted < slots.len() {
            let Some(mut events) = slots[emitted].take() else {
                break;
            };
            if events.is_empty() {
                // The runner filtered this suite out: it reports nothing.
                emitted += 1;
                continue;
            }
            // Replayed and rerun blocks keep their own spacing on this
            // run's clock.
            // The merged clock never runs backwards: a block starts no
            // earlier than the last stamp written.
            let first = events
                .iter()
                .find_map(|event| event.u64_at(&["time"]))
                .unwrap_or(0);
            let now = u64::try_from(started.elapsed().as_millis())
                .unwrap_or(u64::MAX)
                .max(clock);
            report::rebase(&mut events, &mut next_id, now.saturating_sub(first));
            clock = events
                .iter()
                .filter_map(|event| event.u64_at(&["time"]))
                .max()
                .unwrap_or(clock)
                .max(clock);
            let tally = tally(&events);
            totals.passed += tally.passed;
            totals.skipped += tally.skipped;
            totals.failed += tally.failed;
            totals.incomplete |= tally.incomplete;
            outputs.block(&events)?;
            emitted += 1;
        }
        Ok(())
    };
    emit(&mut slots, &mut outputs, &mut totals)?;

    // The suites to run, grouped where the shared program can host them.
    // The facts pass that keyed the run already read every suite: its
    // verdict decides grouping, and only a run without a cache reads again.
    let (grouped, mut stock): (Vec<usize>, Vec<usize>) = to_run.iter().partition(|&&index| {
        let file = &plan.suites[index].abs;
        match cache.as_ref().and_then(|cache| cache.facts(file)) {
            Some(facts) => !facts.launch,
            None => groupable(file),
        }
    });
    let mut staged_keys: FxHashMap<usize, String> = FxHashMap::default();
    let mut settle = |index: usize,
                      block: SuiteBlock,
                      verdict_ok: bool,
                      slots: &mut Vec<Option<Vec<Json>>>,
                      cache: &mut Option<dartq_test::Cache>|
     -> io::Result<()> {
        if verdict_ok {
            if let (Some(cache), Some(key)) = (cache.as_mut(), keys[index].as_deref()) {
                cache.stage(key, &block.events)?;
                staged_keys.insert(index, key.to_owned());
            }
        }
        slots[index] = Some(block.events);
        Ok(())
    };

    if grouped.len() > 1 {
        let out_dir = root.join(".dart_tool").join("dartq").join("group");
        let files: Vec<PathBuf> = grouped
            .iter()
            .map(|&index| plan.suites[index].abs.clone())
            .collect();
        // The degradation ladder's write leg: a read-only checkout (or
        // any unwritable `.dart_tool`) fails the staging writes here —
        // every suite falls back to the stock runner, with the reason
        // on stderr so the slowdown is never a mystery.
        let compiled = match dartq_test::group::compile(
            &plan.dart,
            root,
            &files,
            &out_dir,
            idle_timeout(),
        ) {
            Ok(compiled) => compiled,
            Err(error) => {
                eprintln!(
                    "warning: grouped compile unavailable ({error}); running every suite with the stock runner — this is slower"
                );
                dartq_test::group::Compiled {
                    runner: None,
                    excluded: Vec::new(),
                }
            }
        };
        match compiled.runner {
            None => stock.extend(&grouped),
            Some(program) => {
                let excluded: FxHashSet<&Path> =
                    compiled.excluded.iter().map(PathBuf::as_path).collect();
                let mut hosted: Vec<usize> = Vec::with_capacity(grouped.len());
                for &index in &grouped {
                    if excluded.contains(plan.suites[index].abs.as_path()) {
                        stock.push(index);
                    } else {
                        hosted.push(index);
                    }
                }
                let leftover =
                    run_grouped(plan, &program, &hosted, &index_of, &mut |index, block| {
                        let proven = complete(&block) && tally(&block.events).green;
                        if proven {
                            stats.grouped += 1;
                            settle(index, block, true, &mut slots, &mut cache)?;
                            emit(&mut slots, &mut outputs, &mut totals)
                        } else {
                            rerun.push(index);
                            Ok(())
                        }
                    })?;
                let (leftover, filtered) = leftover;
                rerun.extend(leftover);
                for index in filtered {
                    stats.grouped += 1;
                    let block = SuiteBlock {
                        path: plan.suites[index].spelling.clone(),
                        events: Vec::new(),
                        closed: true,
                        clean: false,
                    };
                    settle(index, block, true, &mut slots, &mut cache)?;
                }
                emit(&mut slots, &mut outputs, &mut totals)?;
            }
        }
    } else {
        stock.extend(&grouped);
    }

    // Everything not proven grouped runs stock, which reports it.
    stock.extend(rerun.drain(..));
    stock.sort_unstable();
    stock.dedup();
    if !stock.is_empty() {
        let leftover = run_stock(plan, &stock, &index_of, &mut |index, block| {
            stats.stock += 1;
            let proven = complete(&block) && tally(&block.events).green;
            settle(index, block, proven, &mut slots, &mut cache)?;
            emit(&mut slots, &mut outputs, &mut totals)
        })?;
        let (lost, loose, spoke) = leftover;
        stock_stderr = spoke;
        for (index, events) in lost {
            stats.stock += 1;
            if events.is_empty() {
                let block = SuiteBlock {
                    path: plan.suites[index].spelling.clone(),
                    events,
                    closed: true,
                    clean: false,
                };
                settle(index, block, true, &mut slots, &mut cache)?;
            } else {
                slots[index] = Some(events);
            }
        }
        emit(&mut slots, &mut outputs, &mut totals)?;
        for event in &loose {
            if event.kind() == "error" {
                totals.failed += 1;
            }
            outputs.each(|sink| sink.loose(event))?;
        }
    }

    // Publish what ran green with unchanged inputs; discard the rest.
    if let Some(cache) = cache.as_mut() {
        let staged: Vec<usize> = staged_keys.keys().copied().collect();
        let files: Vec<PathBuf> = staged
            .iter()
            .map(|&index| plan.suites[index].abs.clone())
            .collect();
        let mut verified: FxHashSet<String> = FxHashSet::default();
        match cache.rekey(&files, runner) {
            Ok(after) => {
                for (index, after) in staged.iter().zip(after) {
                    if after.as_deref() == Some(staged_keys[index].as_str()) {
                        verified.insert(staged_keys[index].clone());
                    }
                }
            }
            Err(error) => eprintln!("dartq test: result cache not updated ({error})"),
        }
        if let Err(error) = cache.commit(&verified).and_then(|()| cache.save()) {
            eprintln!("dartq test: result cache not saved ({error})");
        }
    }

    // The engine's verdict: every test it ran passed (vacuously so when
    // none ran, which the exit code reports separately). A test that never
    // finished fails the run without joining the count.
    let success = totals.failed == 0 && !totals.incomplete;
    let stock_spoke = stock_stderr;
    outputs.each(|sink| sink.done(success))?;
    if plan.inv.stats {
        let refusals = refusal_summary(&refusals);
        let refusals = if refusals.is_empty() {
            String::new()
        } else {
            format!(" ({refusals})")
        };
        eprintln!(
            "dartq test: {} suites — {} replayed, {} grouped, {} stock{} ({:.1}s)",
            plan.suites.len(),
            stats.replayed,
            stats.grouped,
            stats.stock,
            refusals,
            started.elapsed().as_secs_f64()
        );
    }
    if totals.passed + totals.skipped + totals.failed == 0 && !totals.incomplete {
        if !stock_spoke {
            eprintln!("{}", no_tests_message(&plan.inv));
        }
        return Ok(NO_TESTS);
    }
    Ok(if success { exit::CLEAN } else { 1 })
}

/// The runner's `NoTestsFoundException` message (`runner.dart`): the name
/// patterns when any were given, else the tag selectors when any were
/// given, else none.
fn no_tests_message(inv: &Invocation) -> String {
    let patterns: Vec<String> = inv
        .names
        .iter()
        .map(|name| format!("regular expression \"{name}\""))
        .chain(inv.plain_names.iter().map(|name| format!("\"{name}\"")))
        .collect();
    if !patterns.is_empty() {
        let sentence = match patterns.len() {
            1 => patterns[0].clone(),
            2 => format!("{} and {}", patterns[0], patterns[1]),
            n => format!("{}, and {}", patterns[..n - 1].join(", "), patterns[n - 1]),
        };
        return format!("No tests match {sentence}.");
    }
    if !inv.include_tags.is_empty() || !inv.exclude_tags.is_empty() {
        return format!(
            "No tests match the requested tag selectors:\n  include: \"{}\"\n  exclude: \"{}\"",
            tag_selector(&inv.include_tags, "<all>", " && "),
            tag_selector(&inv.exclude_tags, "<none>", " || ")
        );
    }
    "No tests were found.".to_owned()
}

/// A tag selector as `BooleanSelector.toString` writes it: the folded
/// intersection (include) or union (exclude) of every value, each
/// comma-separated list a union of its own.
fn tag_selector(values: &[String], empty: &str, join: &str) -> String {
    let parts: Vec<String> = values
        .iter()
        .flat_map(|value| value.split(','))
        .map(|tag| tag.trim().to_owned())
        .filter(|tag| !tag.is_empty())
        .collect();
    if parts.is_empty() {
        return empty.to_owned();
    }
    parts.join(join)
}

/// The VM binary `dart` on the path launches (what the runner's
/// `Platform.executable` names): the Flutter SDK's `bin/dart` script runs
/// `bin/cache/dart-sdk/bin/dart`; a Dart SDK's `bin/dart` is the VM itself.
fn vm_executable(dart: &Path) -> PathBuf {
    let resolved = dart.canonicalize().unwrap_or_else(|_| dart.to_path_buf());
    let bundled = resolved
        .parent()
        .map(|bin| bin.join("cache").join("dart-sdk").join("bin").join("dart"));
    match bundled {
        Some(vm) if vm.is_file() => vm,
        _ => resolved,
    }
}

fn idle_timeout() -> Duration {
    std::env::var("DARTQ_TEST_IDLE_TIMEOUT")
        .ok()
        .and_then(|secs| secs.parse().ok())
        .map_or(IDLE_TIMEOUT, Duration::from_secs)
}

/// The terminal width (the compact reporter's line), else the runner's
/// default of 200.
fn terminal_width() -> usize {
    #[cfg(unix)]
    {
        // SAFETY: `TIOCGWINSZ` fills the `winsize` this function owns.
        let mut size: libc::winsize = unsafe { std::mem::zeroed() };
        if unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut size) } == 0
            && size.ws_col > 0
        {
            return usize::from(size.ws_col);
        }
    }
    200
}

/// A suite the grouped runner hosts exactly: its own file makes no launch
/// observation (`Platform.script`, `Platform.executableArguments`).
fn groupable(file: &Path) -> bool {
    std::fs::read(file).is_ok_and(|bytes| !dartq_test::deps::facts(&bytes).launch)
}

// ---------------------------------------------------------------------------
// Executors
// ---------------------------------------------------------------------------

/// Concurrent suites per grouped process. Every process carries the whole
/// program, so on a memory-tight host one process with all the
/// concurrency is cheapest; with memory to spare, several processes keep
/// each one's garbage-collection pauses (which stop the whole isolate
/// group) from serializing the run. Measured on a 171-suite package,
/// 10-core host: 7 suites in 1 process 61 s, in 3×3 44 s, in 4×2 45 s.
const SUITES_PER_SHARD: usize = 3;

/// A block's suite, by the path the reporter announced.
fn suite_index(
    block: &SuiteBlock,
    root: &Path,
    index_of: &FxHashMap<&Path, usize>,
) -> Option<usize> {
    let path = Path::new(&block.path);
    let full = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    let full = full.canonicalize().ok()?;
    index_of.get(full.as_path()).copied()
}

/// Run `indices` through the grouped program, in memory-leased shards.
/// Each suite's block reaches `on_block` as the suite settles. Returns the
/// suites no block settled for: those of a shard that died, which run
/// stock, and those a finished shard filtered out, which report nothing.
fn run_grouped(
    plan: &Plan,
    program: &dartq_test::GroupRunner,
    indices: &[usize],
    index_of: &FxHashMap<&Path, usize>,
    on_block: &mut dyn FnMut(usize, SuiteBlock) -> io::Result<()>,
) -> io::Result<(Vec<usize>, Vec<usize>)> {
    let root = &plan.root;
    let profile = dartq_test::Profile::GROUPED.calibrated("grouped");
    let target = plan.workers.min(indices.len()).max(1);
    let wanted = target.div_ceil(SUITES_PER_SHARD);
    let deadline = Instant::now() + idle_timeout();
    let (lease, workers) = dartq_test::lease_processes(profile, target, wanted, deadline)?
        .ok_or_else(|| {
            io::Error::other("timed out waiting for host memory held by other dartq runs")
        })?;
    let (shards, workers) = match &lease {
        Some(lease) => dartq_test::split(profile, lease.units(), target, wanted),
        None => (wanted.min(workers).max(1), workers),
    };
    let per_shard = workers.div_ceil(shards).max(1);
    let durations = dartq_test::group::Durations::load(root);
    let files: Vec<PathBuf> = indices
        .iter()
        .map(|&index| plan.suites[index].abs.clone())
        .collect();
    let parts = dartq_test::group::shard(&files, shards, |suite| durations.get(root, suite));
    let shard_of: FxHashMap<&Path, usize> = parts
        .iter()
        .enumerate()
        .flat_map(|(shard, part)| part.iter().map(move |abs| (abs.as_path(), shard)))
        .collect();
    let mut finished = vec![false; parts.len()];
    let spelled = |part: &[PathBuf]| -> Vec<PathBuf> {
        part.iter()
            .map(|abs| PathBuf::from(&plan.suites[index_of[abs.as_path()]].spelling))
            .collect()
    };

    let (sender, receiver) = std::sync::mpsc::channel::<dartq_test::Streamed>();
    let mut settled: FxHashSet<usize> = FxHashSet::default();
    let mut recorded: Vec<(PathBuf, u64)> = Vec::new();
    let mut peak = 0u64;
    let mut tester_peak = 0u64;
    let mut failure: Option<io::Error> = None;
    std::thread::scope(|scope| {
        let handles: Vec<_> = parts
            .iter()
            .map(|part| {
                let cfg = ExecConfig {
                    dart_bin: plan.dart.clone(),
                    cwd: root.clone(),
                    extra_args: std::iter::once(format!("--concurrency={per_shard}"))
                        .chain(plan.inv.pass.iter().cloned())
                        .collect(),
                    timeout: Some(idle_timeout()),
                    sink: Some(sender.clone()),
                    ready: None,
                };
                let paths = spelled(part);
                scope.spawn(move || dartq_test::execute_grouped(program, &paths, &cfg))
            })
            .collect();
        drop(sender);
        for streamed in receiver {
            // A raw line of no suite in flight prints where the runner
            // wrote it; every other item is a settled suite's block.
            let dartq_test::Streamed::Settled(Settled { block, .. }) = streamed else {
                if let dartq_test::Streamed::Line(line) = streamed {
                    println!("{line}");
                }
                continue;
            };
            if failure.is_some() {
                continue;
            }
            let Some(index) = suite_index(&block, root, index_of) else {
                continue;
            };
            if let Some(ms) = block_duration(&block.events) {
                recorded.push((
                    plan.suites[index]
                        .abs
                        .strip_prefix(root)
                        .unwrap_or(&plan.suites[index].abs)
                        .to_path_buf(),
                    ms,
                ));
            }
            settled.insert(index);
            if let Err(error) = on_block(index, block) {
                failure = Some(error);
            }
        }
        for (shard, handle) in handles.into_iter().enumerate() {
            if let Ok(Ok(execution)) = handle.join() {
                peak += execution.peak_rss;
                tester_peak = tester_peak.max(execution.peak_tester_rss);
                finished[shard] = execution.complete;
            }
        }
    });
    drop(lease);
    profile.observe("grouped", peak, tester_peak, workers, parts.len());
    durations.record(root, &recorded);
    if let Some(error) = failure {
        return Err(error);
    }
    // As under the stock runner: a finished shard filtered out every suite
    // it did not report; a dead shard's unreported suites rerun stock.
    let mut filtered = Vec::new();
    let mut leftover = Vec::new();
    for &index in indices.iter().filter(|index| !settled.contains(index)) {
        if finished[shard_of[plan.suites[index].abs.as_path()]] {
            filtered.push(index);
        } else {
            leftover.push(index);
        }
    }
    Ok((leftover, filtered))
}

/// The span between a block's first and last `time` stamp.
fn block_duration(events: &[Json]) -> Option<u64> {
    let first = events.iter().find_map(|event| event.u64_at(&["time"]))?;
    let last = events
        .iter()
        .rev()
        .find_map(|event| event.u64_at(&["time"]))?;
    Some(last.saturating_sub(first))
}

/// Run `indices` under the stock runner (one leased invocation, the
/// runner's own concurrency). Blocks reach `on_block` as their suites
/// settle; returns, for each suite the runner never reported, its final
/// events: none when the run finished (the runner filtered the suite
/// out), else the load-failure block the stock runner prints for a
/// crashed suite.
fn run_stock(
    plan: &Plan,
    indices: &[usize],
    index_of: &FxHashMap<&Path, usize>,
    on_block: &mut dyn FnMut(usize, SuiteBlock) -> io::Result<()>,
) -> io::Result<(Vec<(usize, Vec<Json>)>, Vec<Json>, bool)> {
    let root = &plan.root;
    let deadline = Instant::now() + idle_timeout();
    let (lease, workers) = dartq_test::lease_run(plan.workers.min(indices.len()).max(1), deadline)?
        .ok_or_else(|| {
            io::Error::other("timed out waiting for host memory held by other dartq runs")
        })?;
    let (sender, receiver) = std::sync::mpsc::channel::<dartq_test::Streamed>();
    let cfg = ExecConfig {
        dart_bin: plan.dart.clone(),
        cwd: root.clone(),
        extra_args: std::iter::once(format!("--concurrency={workers}"))
            .chain(plan.inv.pass.iter().cloned())
            .collect(),
        timeout: Some(idle_timeout()),
        sink: Some(sender),
        ready: None,
    };
    let paths: Vec<PathBuf> = indices
        .iter()
        .map(|&index| PathBuf::from(&plan.suites[index].spelling))
        .collect();
    let mut settled: FxHashSet<usize> = FxHashSet::default();
    let mut failure: Option<io::Error> = None;
    let mut ran: Option<Result<dartq_test::Execution, dartq_test::ExecError>> = None;
    std::thread::scope(|scope| {
        // `cfg` moves into the runner thread: its sender must drop with the
        // run, or the receive loop below would wait forever.
        let paths = &paths;
        let handle = scope.spawn(move || dartq_test::execute(paths, &cfg));
        for streamed in &receiver {
            // A raw line of no suite in flight prints where the runner
            // wrote it; every other item is a settled suite's block.
            let dartq_test::Streamed::Settled(Settled { block, .. }) = streamed else {
                if let dartq_test::Streamed::Line(line) = streamed {
                    println!("{line}");
                }
                continue;
            };
            if failure.is_some() {
                continue;
            }
            let Some(index) = suite_index(&block, root, index_of) else {
                continue;
            };
            settled.insert(index);
            if let Err(error) = on_block(index, block) {
                failure = Some(error);
            }
        }
        ran = handle.join().ok();
    });
    drop(lease);
    if let Some(error) = failure {
        return Err(error);
    }
    let mut events = Vec::new();
    let mut spoke = false;
    let finished = matches!(&ran, Some(Ok(execution)) if execution.complete);
    match ran {
        Some(Ok(execution)) => {
            // Runner-level output goes where the runner wrote it: events
            // into the report, raw lines to stdout, stderr to stderr.
            for loose in execution.loose {
                match loose {
                    dartq_test::Loose::Event(event) => events.push(event),
                    dartq_test::Loose::Line(line) => println!("{line}"),
                }
            }
            eprint!("{}", execution.stderr);
            spoke = !execution.stderr.is_empty();
        }
        Some(Err(error)) => eprintln!("dartq test: the test runner failed: {error}"),
        None => eprintln!("dartq test: the test runner thread panicked"),
    }
    // A run that reached `done` reported every suite it loaded: a suite it
    // never reported was filtered out by the runner (suite-level tags, a
    // `@TestOn` that excludes this platform) and reports nothing. A run
    // that died before `done` lost its unreported suites.
    let lost = indices
        .iter()
        .copied()
        .filter(|index| !settled.contains(index))
        .map(|index| {
            (
                index,
                if finished {
                    Vec::new()
                } else {
                    lost_suite(&plan.suites[index].spelling)
                },
            )
        })
        .collect();
    Ok((lost, events, spoke))
}

/// The block for a suite whose runner died before reporting it: a failed
/// load, which is what `dart test` reports for a suite it could not run.
fn lost_suite(spelling: &str) -> Vec<Json> {
    let line = |text: String| dartq_test::event::parse(&text).expect("a well-formed event");
    let path = serde_json::to_string(spelling).expect("a string serializes");
    let name = serde_json::to_string(&format!("loading {spelling}")).expect("a string serializes");
    let error = serde_json::to_string(&format!(
        "Failed to load \"{spelling}\": the test runner exited before the suite reported"
    ))
    .expect("a string serializes");
    vec![
        line(format!(r#"{{"suite":{{"id":0,"platform":"vm","path":{path}}},"type":"suite","time":0}}"#)),
        line(format!(
            r#"{{"test":{{"id":1,"name":{name},"suiteID":0,"groupIDs":[],"metadata":{{"skip":false,"skipReason":null}},"line":null,"column":null,"url":null}},"type":"testStart","time":0}}"#
        )),
        line(format!(r#"{{"testID":1,"error":{error},"stackTrace":"","isFailure":false,"type":"error","time":0}}"#)),
        line(r#"{"testID":1,"result":"error","skipped":false,"hidden":false,"type":"testDone","time":0}"#.to_owned()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn args(line: &[&str]) -> Vec<String> {
        line.iter().map(|arg| (*arg).to_owned()).collect()
    }

    /// The runner's grammar: options anywhere, `=` or separate values,
    /// short options, `--no-` negation, `--` ending options; dartq's own
    /// options are recognized and kept from the runner.
    #[test]
    fn grammar_matches_the_runner() {
        let inv = parse(&args(&[
            "test/a_test.dart",
            "-r",
            "expanded",
            "-j4",
            "--name=foo",
            "-N",
            "bar baz",
            "--no-color",
            "--chain-stack-traces",
            "--",
            "-weird_test.dart",
        ]))
        .expect("parse");
        assert_eq!(inv.paths, ["test/a_test.dart", "-weird_test.dart"]);
        assert_eq!(inv.reporter, Some(Kind::Expanded));
        assert_eq!(inv.concurrency, Some(4));
        assert_eq!(inv.names, ["foo"]);
        assert_eq!(inv.plain_names, ["bar baz"]);
        assert_eq!(inv.color, Some(false));
        assert!(inv.chain_stack_traces);
        assert_eq!(
            inv.pass,
            ["--name=foo", "--plain-name=bar baz", "--chain-stack-traces"]
        );
        assert!(!inv.delegate);

        let inv = parse(&args(&[
            "-t",
            "slow",
            "--exclude-tags=flaky",
            "--timeout",
            "2x",
            "--total-shards=3",
            "--shard-index=1",
            "--file-reporter",
            "json:out/r.json",
        ]))
        .expect("parse");
        assert_eq!(
            inv.pass,
            [
                "--tags=slow",
                "--exclude-tags=flaky",
                "--timeout=2x",
                "--total-shards=3",
                "--shard-index=1"
            ]
        );
        assert_eq!(
            inv.file_reporter,
            Some((Kind::Json, PathBuf::from("out/r.json")))
        );

        let inv = parse(&args(&["--impacted", "--no-cache", "--stats"])).expect("parse");
        assert_eq!(inv.impacted.as_deref(), Some("HEAD"));
        assert!(inv.no_cache && inv.stats);
        assert_eq!(
            parse(&args(&["--impacted=main"]))
                .expect("parse")
                .impacted
                .as_deref(),
            Some("main")
        );
        assert_eq!(
            runner_args(&args(&[
                "-r",
                "json",
                "--no-cache",
                "--impacted=main",
                "--",
                "--stats"
            ])),
            ["-r", "json", "--", "--stats"]
        );

        // Only the runner reproduces these: delegated.
        for line in [
            &["--help"][..],
            &["-h"],
            &["--version"],
            &["--debug"],
            &["--coverage=cov"],
            &["--fail-fast"],
            &["-p", "chrome"],
            &["--test-randomize-ordering-seed=1"],
            &["-j", "0"],
            &["--pause-after-load"],
        ] {
            assert!(parse(&args(line)).expect("parse").delegate, "{line:?}");
        }
        assert!(
            !parse(&args(&["--no-fail-fast", "-p", "vm"]))
                .expect("parse")
                .delegate
        );
        // Lines the runner rejects are its to report.
        for line in [
            &["--bogus"][..],
            &["-r", "nope"],
            &["-r"],
            &["--color=yes"],
            &["-j", "x"],
            &["--file-reporter", "json"],
            &["--no-name"],
        ] {
            assert!(parse(&args(line)).is_err(), "{line:?}");
        }
    }

    /// Selections are keyed as URI paths: dot segments removed, a
    /// leading `..` kept, URI syntax refused.
    #[test]
    fn spellings_match_the_runner() {
        assert_eq!(spell("test").as_deref(), Some("test"));
        assert_eq!(spell("./test").as_deref(), Some("test"));
        assert_eq!(spell("test/./sub").as_deref(), Some("test/sub"));
        assert_eq!(
            spell("test/sub/../sub/b_test.dart").as_deref(),
            Some("test/sub/b_test.dart")
        );
        assert_eq!(spell("test/").as_deref(), Some("test/"));
        assert_eq!(spell("../other/test").as_deref(), Some("../other/test"));
        assert_eq!(
            spell("/abs/./x_test.dart").as_deref(),
            Some("/abs/x_test.dart")
        );
        assert_eq!(spell("test//sub").as_deref(), Some("test//sub"));
        assert_eq!(spell("a_test.dart?name=x"), None);
        assert_eq!(spell("a%20b_test.dart"), None);
    }

    /// The `--stats` refusal histogram: `count label` entries, most
    /// frequent first (ties alphabetical), empty when nothing refused —
    /// the line wraps it in parentheses only when non-empty.
    #[test]
    fn refusal_summary_is_stable() {
        use dartq_test::Refusal;
        assert_eq!(refusal_summary(&[]), "");
        assert_eq!(refusal_summary(&[None, None]), "");
        let refusals = [
            Some(Refusal::Nondeterministic),
            Some(Refusal::AbsoluteDirectory),
            Some(Refusal::Nondeterministic),
            None,
            Some(Refusal::AbsoluteDirectory),
            Some(Refusal::Nondeterministic),
            Some(Refusal::EnvOpaque),
        ];
        assert_eq!(
            refusal_summary(&refusals),
            "3 nondeterministic, 2 absolute-directory, 1 environment"
        );
        // Equal counts sort by label; keyed and refused suites mix freely.
        let tied = [Some(Refusal::Untrusted), Some(Refusal::NoCache)];
        assert_eq!(refusal_summary(&tied), "1 no-cache, 1 untrusted");
    }

    /// `flutter test`'s own grammar: the tool's shaping options pass,
    /// dartq's `--no-pub`/`--machine` are recognized, `-j` follows the
    /// tool's lenient `int.tryParse`, and only the tester platform runs
    /// here.
    #[test]
    fn flutter_grammar_matches_the_tool() {
        let inv = parse(&args(&[
            "--flutter",
            "--no-pub",
            "--machine",
            "-j",
            "4",
            "--dart-define=A=1",
            "--update-goldens",
            "test/widget_test.dart",
        ]))
        .expect("parse");
        assert!(inv.flutter && inv.no_pub && inv.machine && !inv.delegate);
        assert_eq!(inv.concurrency, Some(4));
        assert_eq!(inv.pass, ["--dart-define=A=1", "--update-goldens"]);
        assert!(inv.no_cache, "a run that writes goldens never caches");
        assert_eq!(inv.paths, ["test/widget_test.dart"]);

        // The tester platform runs here (the tool's default, so it is not
        // re-passed); anything else is the tool's.
        let tester = parse(&args(&["--flutter", "--platform", "tester"])).expect("parse");
        assert!(!tester.delegate && tester.pass.is_empty());
        assert!(
            parse(&args(&["--flutter", "--platform", "chrome"]))
                .expect("parse")
                .delegate
        );

        // The tool's own lenient parse: junk leaves the default and the
        // run stays dartq's.
        let junk = parse(&args(&["--flutter", "-j", "x"])).expect("parse");
        assert!(!junk.delegate && junk.concurrency.is_none());

        // Tag options accumulate; debugging, verbose logging and coverage
        // delegate.
        let tags =
            parse(&args(&["--flutter", "-t", "slow", "--exclude-tags=flaky"])).expect("parse");
        assert_eq!(tags.include_tags, ["slow"]);
        assert_eq!(tags.exclude_tags, ["flaky"]);
        for line in [
            &["--flutter", "--verbose"][..],
            &["--flutter", "-v"],
            &["--flutter", "--start-paused"],
            &["--flutter", "--coverage"],
        ] {
            assert!(parse(&args(line)).expect("parse").delegate, "{line:?}");
        }
        // After `--` the options ended: `--flutter` is a selection.
        let after = parse(&args(&["--", "--flutter"])).expect("parse");
        assert!(!after.flutter && after.paths == ["--flutter"]);
    }

    /// The runner's `NoTestsFoundException` message: name patterns first,
    /// then the tag selectors, else the plain sentence.
    #[test]
    fn no_tests_message_matches_the_runner() {
        let own = |values: &[&str]| {
            values
                .iter()
                .map(|value| (*value).to_owned())
                .collect::<Vec<_>>()
        };
        let inv = Invocation {
            names: own(&["a"]),
            ..Invocation::default()
        };
        assert_eq!(
            no_tests_message(&inv),
            "No tests match regular expression \"a\"."
        );
        let inv = Invocation {
            names: own(&["a", "b"]),
            ..Invocation::default()
        };
        assert_eq!(
            no_tests_message(&inv),
            "No tests match regular expression \"a\" and regular expression \"b\"."
        );
        let inv = Invocation {
            names: own(&["a", "b", "c"]),
            ..Invocation::default()
        };
        assert_eq!(
            no_tests_message(&inv),
            "No tests match regular expression \"a\", regular expression \"b\", and regular expression \"c\"."
        );
        let inv = Invocation {
            plain_names: own(&["x y"]),
            ..Invocation::default()
        };
        assert_eq!(no_tests_message(&inv), "No tests match \"x y\".");
        let inv = Invocation {
            include_tags: own(&["s", "t,u"]),
            exclude_tags: own(&["v"]),
            ..Invocation::default()
        };
        assert_eq!(
            no_tests_message(&inv),
            "No tests match the requested tag selectors:\n  include: \"s && t && u\"\n  exclude: \"v\""
        );
        assert_eq!(
            no_tests_message(&Invocation::default()),
            "No tests were found."
        );
    }

    /// The rerun hint names the VM the `dart` entry point launches: a
    /// Flutter SDK's shim names the VM in its cache, a Dart SDK's `dart`
    /// is the VM itself.
    #[test]
    fn rerun_hints_name_the_underlying_vm() {
        let tmp = tempfile::tempdir().expect("tmp");
        let shim = tmp.path().join("sdk/bin/dart");
        std::fs::create_dir_all(shim.parent().expect("parent")).expect("dir");
        std::fs::write(&shim, "shim").expect("write");
        let vm = tmp.path().join("sdk/bin/cache/dart-sdk/bin/dart");
        std::fs::create_dir_all(vm.parent().expect("parent")).expect("dir");
        std::fs::write(&vm, "vm").expect("write");
        assert_eq!(vm_executable(&shim), vm.canonicalize().expect("canon"));
        let plain = tmp.path().join("dart");
        std::fs::write(&plain, "vm").expect("write");
        assert_eq!(vm_executable(&plain), plain.canonicalize().expect("canon"));
    }

    /// A skipped test that errored counts as failed: errors outrank skips.
    #[test]
    fn an_errored_skip_fails() {
        let event = |line: &str| dartq_test::event::parse(line).expect("event");
        let rows = vec![
            event(r#"{"suite":{"id":0,"path":"test/a_test.dart"},"type":"suite","time":0}"#),
            event(
                r#"{"test":{"id":1,"name":"t","suiteID":0,"groupIDs":[],"metadata":{"skip":true}},"type":"testStart","time":0}"#,
            ),
            event(
                r#"{"testID":1,"error":"boom","stackTrace":"","isFailure":false,"type":"error","time":1}"#,
            ),
            event(
                r#"{"testID":1,"result":"success","skipped":true,"hidden":false,"type":"testDone","time":1}"#,
            ),
        ];
        let t = tally(&rows);
        assert!(!t.green && t.failed == 1 && t.skipped == 0 && t.passed == 0);
    }

    /// Discovery walks hidden and `build` directories and links, as the
    /// runner's `listSync(recursive: true)` does; a file reached twice or a
    /// missing selection is the runner's to report.
    #[test]
    fn discovery_matches_the_runner() {
        let tmp = tempfile::tempdir().expect("tmp");
        let root = tmp.path().canonicalize().expect("canon");
        for rel in [
            "test/a_test.dart",
            "test/.hidden/b_test.dart",
            "test/build/c_test.dart",
            "test/helper.dart",
            "test/sub/d_test.dart",
        ] {
            let path = root.join(rel);
            fs::create_dir_all(path.parent().expect("parent")).expect("dir");
            fs::write(path, "void main() {}\n").expect("write");
        }
        let names = |suites: Vec<Suite>| {
            let mut names: Vec<String> = suites.into_iter().map(|suite| suite.spelling).collect();
            names.sort();
            names
        };
        let all = discover(&root, &["test".to_owned()], None).expect("discover");
        assert_eq!(
            names(all),
            [
                "test/.hidden/b_test.dart",
                "test/a_test.dart",
                "test/build/c_test.dart",
                "test/sub/d_test.dart"
            ]
        );
        let narrowed = discover(
            &root,
            &["./test/sub".to_owned(), "test/a_test.dart".to_owned()],
            None,
        )
        .expect("discover");
        assert_eq!(
            narrowed
                .iter()
                .map(|suite| suite.spelling.as_str())
                .collect::<Vec<_>>(),
            ["test/sub/d_test.dart", "test/a_test.dart"],
            "selection order"
        );
        let globbed = discover(&root, &["test".to_owned()], Some("*.dart")).expect("discover");
        assert_eq!(globbed.len(), 5);
        assert!(discover(&root, &["test/missing_test.dart".to_owned()], None).is_none());
        assert!(
            discover(
                &root,
                &["test".to_owned(), "test/a_test.dart".to_owned()],
                None
            )
            .is_none(),
            "reached twice"
        );
    }

    /// `dart_test.yaml`: selection keys are read; keys whose effect only
    /// the runner reproduces delegate the run.
    #[test]
    fn file_config_reads_selection_keys() {
        let tmp = tempfile::tempdir().expect("tmp");
        let root = tmp.path();
        assert!(file_config(root).expect("absent").paths.is_none());
        fs::write(root.join("dart_test.yaml"), "paths: [test/unit]\nfilename: \"*_spec.dart\"\nconcurrency: 3\ntags:\n  slow: {timeout: 2x}\n").expect("write");
        let config = file_config(root).expect("config");
        assert_eq!(config.paths, Some(vec!["test/unit".to_owned()]));
        assert_eq!(config.filename.as_deref(), Some("*_spec.dart"));
        assert_eq!(config.concurrency, Some(3));
        fs::write(root.join("dart_test.yaml"), "reporter: expanded\n").expect("write");
        assert!(file_config(root).is_err());
        fs::write(root.join("dart_test.yaml"), "presets: {ci: {}}\n").expect("write");
        assert!(file_config(root).is_err());
    }

    /// A delegation names the runner it hands the run to and the reason on
    /// one line.
    #[test]
    fn delegations_name_the_runner_and_reason() {
        assert_eq!(
            delegation_line(false, "no pubspec.yaml here"),
            "dartq: delegating to dart test — no pubspec.yaml here"
        );
        assert_eq!(
            delegation_line(true, "no flutter on PATH"),
            "dartq: delegating to flutter test — no flutter on PATH"
        );
    }

    /// State delegations name the missing precondition, in the gate's own
    /// order: the pubspec, the resolved package configuration, the global
    /// test configuration.
    #[test]
    fn state_reasons_name_the_missing_precondition() {
        let tmp = tempfile::tempdir().expect("tmp");
        let root = tmp.path();
        assert_eq!(
            state_reason(root, false).as_deref(),
            Some("no pubspec.yaml here")
        );
        fs::write(root.join("pubspec.yaml"), "name: p\n").expect("write");
        let reason = state_reason(root, false).expect("stale");
        assert!(reason.contains("package_config.json"), "{reason}");
        assert!(reason.contains("run dart pub get"), "{reason}");
        fs::create_dir_all(root.join(".dart_tool")).expect("dir");
        fs::write(
            root.join(".dart_tool/package_config.json"),
            r#"{"configVersion":2,"packages":[]}"#,
        )
        .expect("write");
        assert_eq!(
            state_reason(root, false),
            None,
            "a resolved, config-less package runs here"
        );
        assert_eq!(
            state_reason(root, true).as_deref(),
            Some("global dart_test config present")
        );
    }

    /// A suite is green only when nothing errored, the load included, and
    /// it is complete only when every started test finished.
    #[test]
    fn verdicts_follow_the_events() {
        let event = |line: &str| dartq_test::event::parse(line).expect("event");
        let ok = vec![
            event(r#"{"suite":{"id":0,"path":"test/a_test.dart"},"type":"suite","time":0}"#),
            event(
                r#"{"test":{"id":1,"name":"loading test/a_test.dart","suiteID":0,"groupIDs":[]},"type":"testStart","time":0}"#,
            ),
            event(
                r#"{"testID":1,"result":"success","skipped":false,"hidden":true,"type":"testDone","time":1}"#,
            ),
            event(
                r#"{"test":{"id":3,"name":"t","suiteID":0,"groupIDs":[2]},"type":"testStart","time":1}"#,
            ),
            event(
                r#"{"testID":3,"result":"success","skipped":false,"hidden":false,"type":"testDone","time":2}"#,
            ),
        ];
        let t = tally(&ok);
        assert!(t.green && t.passed == 1 && t.failed == 0);
        let block = SuiteBlock {
            path: "test/a_test.dart".into(),
            events: ok.clone(),
            closed: true,
            clean: false,
        };
        assert!(complete(&block));
        let cut = SuiteBlock {
            path: "test/a_test.dart".into(),
            events: ok[..4].to_vec(),
            closed: true,
            clean: false,
        };
        assert!(!complete(&cut), "a started test never finished");
        let mut late = ok.clone();
        late.push(event(r#"{"testID":3,"error":"late","stackTrace":"","isFailure":false,"type":"error","time":3}"#));
        let t = tally(&late);
        assert!(
            !t.green && t.failed == 1 && t.passed == 0,
            "an error after success fails the test"
        );
        let lost = lost_suite("test/a_test.dart");
        assert_eq!(tally(&lost).failed, 1);
        // A test that started and never finished (a suite that called
        // `exit()` mid-test) keeps its outcome unknown: not counted, and
        // the run cannot succeed.
        let cut = tally(&ok[..4]);
        assert!(cut.incomplete, "a started test that never finished");
        assert!(cut.passed == 0 && cut.failed == 0, "it counts nowhere");
        assert!(!tally(&ok).incomplete, "a finished suite is complete");
    }
}
