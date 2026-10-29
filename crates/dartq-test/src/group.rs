//! The grouped runner: the stock `package:test` runner with one step
//! replaced — how a suite's isolate comes to exist.
//!
//! ## Why
//!
//! `dart test` compiles every suite into its own kernel file through one
//! serial `frontend_server` and starts each from that file with
//! `Isolate.spawnUri`: a fresh isolate *group*, so every suite re-loads and
//! re-JIT-compiles the SDK, `package:test`, the package under test and its
//! dependencies from scratch. Measured on a 171-suite package (10-core
//! host): 136 s of serial load time, and the first test of each suite —
//! where that cold code warms up — costing as much as every other test
//! combined (305 s vs 297 s).
//!
//! Here every suite is compiled once into a single program (one kernel
//! compile, ~5 s for those 171 suites) and started with `Isolate.spawn`:
//! a new isolate in the *same* group, which shares the loaded program and
//! its JIT-compiled code but none of its state — static fields, zones and
//! microtask queues are per isolate, exactly as with `spawnUri`. Suite
//! load drops to a 13 ms median, and warm code is shared across suites.
//!
//! ## What stays stock
//!
//! The generated entry point registers one [`PlatformPlugin`] for the VM
//! runtime and then calls the stock runner's own `main`: configuration,
//! `dart_test.yaml`, tag and platform selection, suite metadata, the
//! engine, timeouts, retries and the JSON reporter are all `package:test`'s
//! own. The plugin delegates to the stock `VMPlatform` for anything it does
//! not handle (a suite missing from the program, debugging, coverage,
//! pause-after-load).
//!
//! ## What differs, and how the caller stays exact
//!
//! - `Platform.script` names the shared program, not the suite's bootstrap;
//!   `dart:mirrors` sees every suite's libraries. Suites reaching either
//!   ([`crate::deps::FileFacts::launch`]) are run by the stock runner.
//! - The whole program compiles or nothing does: a compile error in any
//!   suite fails the build, and the caller runs everything stock.
//! - One shared heap: a suite that exhausts memory or calls `exit` takes
//!   its whole shard down. The shard's reporter never completes, and the
//!   caller reruns that shard's suites stock.
//!
//! The caller treats the grouped run as a fast first pass: green suites are
//! final; every suite not provably green reruns under the stock runner, so
//! each failure is reported exactly as `dart test` reports it.

use std::fmt::Write as _;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// A compiled grouped runner.
#[derive(Debug, Clone)]
pub struct GroupRunner {
    /// The kernel file holding the runner and every suite.
    pub kernel: PathBuf,
    /// The package configuration the program was compiled against.
    pub packages: PathBuf,
}

impl GroupRunner {
    /// The command-line prefix starting this runner with `dart`: asserts on
    /// (`package:test` spawns suites with `checked: true`), the package
    /// configuration the program was compiled against, then the kernel.
    #[must_use]
    pub fn args(&self) -> Vec<String> {
        vec![
            "--enable-asserts".to_owned(),
            format!("--packages={}", self.packages.display()),
            self.kernel.display().to_string(),
        ]
    }
}

/// The outcome of [`compile`].
#[derive(Debug)]
pub struct Compiled {
    /// The runner hosting every suite not in [`Self::excluded`]; `None`
    /// when no program compiled.
    pub runner: Option<GroupRunner>,
    /// Suites left out of the program because the compiler reported an
    /// error inside the suite file itself. The stock runner reports that
    /// error exactly as `dart test` does.
    pub excluded: Vec<PathBuf>,
}

/// Generate and compile the grouped runner for `suites` (absolute,
/// canonical paths) of the package at `root` into `out_dir`.
///
/// A suite whose own file fails to compile is dropped from the program
/// ([`Compiled::excluded`]) and the rest recompile once, incrementally
/// from the last good kernel (about a second): an edit-test loop with one
/// broken suite keeps every other suite grouped. An error anywhere else (a
/// helper or library several suites share, a `package:test` version
/// without the runner internals the entry point uses) yields no runner:
/// the caller runs the stock runner instead.
///
/// ## Incremental compile
///
/// The program lives at one fixed path per checkout (`group.dart` →
/// `group.dill`), and is compiled by the SDK's own `frontend_server`
/// initialized from the previous `group.dill` — the mechanism `dart test`
/// itself uses for its `.dart_tool/test/incremental_kernel` cache. Before
/// reusing a library from that kernel, `frontend_server` compares the
/// library's source bytes recorded in the kernel against the bytes on disk,
/// byte for byte, and recompiles every library whose bytes differ, that has
/// disappeared, or whose package resolution changed; any library it cannot
/// vouch for is recompiled. On the 171-suite package a one-file edit
/// recompiles in 0.9 s instead of 4.9 s, at 0.5 GB instead of 0.6 GB.
///
/// Without a `frontend_server` (an SDK that does not ship one), or when it
/// fails without reporting compile errors (a crash, an unreadable previous
/// kernel), the program is compiled from scratch with `dart compile
/// kernel`; a failure there means the program does not compile. Reported
/// compile errors are never retried from scratch: the source is wrong, and
/// a second compiler would only find the same errors.
///
/// # Errors
///
/// I/O failures writing the entry point or spawning the compiler.
pub fn compile(
    dart: &Path,
    root: &Path,
    suites: &[PathBuf],
    out_dir: &Path,
    timeout: Duration,
) -> io::Result<Compiled> {
    let packages = crate::cache::package_config_path(root);
    let none = |excluded| Compiled {
        runner: None,
        excluded,
    };
    if !packages.is_file() || suites.is_empty() {
        return Ok(none(Vec::new()));
    }
    std::fs::create_dir_all(out_dir)?;
    let entry = out_dir.join("group.dart");
    let kernel = out_dir.join("group.dill");
    let staged = out_dir.join("group.next.dill");
    let deadline = Instant::now() + timeout;
    let server = frontend_server(dart);
    let mut hosted: Vec<PathBuf> = suites.to_vec();
    let mut excluded: Vec<PathBuf> = Vec::new();
    loop {
        let source = entry_point(out_dir, &hosted);
        if std::fs::read(&entry).ok().as_deref() != Some(source.as_bytes()) {
            crate::stamp::write_atomic(&entry, source.as_bytes())?;
        }
        let _ = std::fs::remove_file(&staged);
        let mut result = match &server {
            Some(server) => run(
                server.incremental(&packages, &entry, &staged, &kernel),
                root,
                deadline,
            )?,
            None => Outcome::Failed,
        };
        if matches!(result, Outcome::Failed) {
            result = run(scratch(dart, &packages, &entry, &staged), root, deadline)?;
        }
        match result {
            Outcome::Compiled if staged.is_file() => {
                // The new kernel replaces the old one only once it is whole,
                // so a concurrent run of the same checkout never reads a
                // half-written file.
                std::fs::rename(&staged, &kernel)?;
                return Ok(Compiled {
                    runner: Some(GroupRunner { kernel, packages }),
                    excluded,
                });
            }
            Outcome::Errors(output) if excluded.is_empty() => {
                // One retry: drop the suites whose own file failed, as long
                // as every error lies in one of them.
                let broken = broken_suites(root, &hosted, &output);
                let _ = std::fs::remove_file(&staged);
                let Some(broken) = broken.filter(|broken| broken.len() < hosted.len()) else {
                    return Ok(none(Vec::new()));
                };
                hosted.retain(|suite| !broken.contains(suite));
                excluded = broken;
            }
            _ => {
                let _ = std::fs::remove_file(&staged);
                return Ok(none(Vec::new()));
            }
        }
    }
}

/// The suites of `hosted` that `output` (compiler stdout) reports errors
/// in; `None` when any reported error lies outside every suite file, or
/// when no error location can be read at all.
///
/// Both compilers print each error as `<path>:<line>:<column>: Error: …`
/// with `<path>` relative to the working directory (`root`) or absolute.
pub(crate) fn broken_suites(root: &Path, hosted: &[PathBuf], output: &str) -> Option<Vec<PathBuf>> {
    let suites: std::collections::HashSet<&Path> = hosted.iter().map(PathBuf::as_path).collect();
    let mut broken: Vec<PathBuf> = Vec::new();
    for line in output.lines() {
        let Some((location, _)) = line.split_once(": Error: ") else {
            continue;
        };
        let mut parts = location.rsplitn(3, ':');
        let (Some(column), Some(row), Some(path)) = (parts.next(), parts.next(), parts.next())
        else {
            return None;
        };
        if column.parse::<u32>().is_err() || row.parse::<u32>().is_err() {
            return None;
        }
        let path = path.strip_prefix("file://").unwrap_or(path);
        let path = root.join(path);
        if !suites.contains(path.as_path()) {
            return None;
        }
        if !broken.contains(&path) {
            broken.push(path);
        }
    }
    (!broken.is_empty()).then_some(broken)
}

/// The SDK's `frontend_server`, run by its AOT runtime.
struct FrontendServer {
    runtime: PathBuf,
    snapshot: PathBuf,
    sdk: PathBuf,
}

/// The `frontend_server` of the SDK `dart` belongs to: `<sdk>/bin/dart`
/// (resolving `fvm`/Flutter `bin/dart` shims to `bin/cache/dart-sdk`).
fn frontend_server(dart: &Path) -> Option<FrontendServer> {
    let dart = dart.canonicalize().ok()?;
    let bin = dart.parent()?;
    let candidates = [
        bin.parent()?.to_path_buf(),
        bin.join("cache").join("dart-sdk"),
    ];
    candidates.into_iter().find_map(|sdk| {
        let runtime = sdk.join("bin").join(if cfg!(windows) {
            "dartaotruntime.exe"
        } else {
            "dartaotruntime"
        });
        let snapshot = sdk
            .join("bin")
            .join("snapshots")
            .join("frontend_server_aot.dart.snapshot");
        let platform = sdk
            .join("lib")
            .join("_internal")
            .join("vm_platform_strong.dill");
        (runtime.is_file() && snapshot.is_file() && platform.is_file()).then_some(FrontendServer {
            runtime,
            snapshot,
            sdk,
        })
    })
}

impl FrontendServer {
    /// One incremental compile of `entry` into `output`, initialized from
    /// `previous` when it exists. Asserts stay in the kernel (`dart test`
    /// compiles with them enabled; the VM runs them with `--enable-asserts`).
    fn incremental(
        &self,
        packages: &Path,
        entry: &Path,
        output: &Path,
        previous: &Path,
    ) -> Command {
        let internal = self.sdk.join("lib").join("_internal");
        let mut cmd = Command::new(&self.runtime);
        cmd.arg(&self.snapshot)
            .arg("--sdk-root")
            .arg(&internal)
            .arg("--platform")
            .arg(internal.join("vm_platform_strong.dill"))
            .args([
                "--target",
                "vm",
                "--incremental",
                "--no-print-incremental-dependencies",
            ])
            .arg(format!("--packages={}", packages.display()))
            .arg("--output-dill")
            .arg(output);
        if previous.is_file() {
            cmd.arg("--initialize-from-dill").arg(previous);
        }
        cmd.arg(entry);
        cmd
    }
}

/// A from-scratch compile with the toolchain's own `dart compile kernel`.
fn scratch(dart: &Path, packages: &Path, entry: &Path, output: &Path) -> Command {
    let mut cmd = Command::new(dart);
    cmd.args(["compile", "kernel"])
        .arg(format!("--packages={}", packages.display()))
        .arg("--output")
        .arg(output)
        .arg(entry);
    cmd
}

/// How one compiler run ended.
enum Outcome {
    Compiled,
    /// The compiler reported compile errors; its combined output.
    Errors(String),
    /// Neither: a crash, a timeout, a non-zero exit with no error line.
    Failed,
}

/// Run a compiler to completion before `deadline`. `frontend_server`
/// reports compile errors on stdout as `<boundary-key> <output> <error
/// count>` and still exits 0, so its result line is parsed rather than
/// trusting the exit status alone; `dart compile kernel` signals them by
/// exit status, with `Error:` lines on its output.
fn run(mut cmd: Command, root: &Path, deadline: Instant) -> io::Result<Outcome> {
    let mut child = cmd
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let drain = |stream: Option<Box<dyn io::Read + Send>>| {
        std::thread::spawn(move || {
            let mut text = String::new();
            if let Some(mut stream) = stream {
                let _ = stream.read_to_string(&mut text);
            }
            text
        })
    };
    let stdout = drain(
        child
            .stdout
            .take()
            .map(|s| Box::new(s) as Box<dyn io::Read + Send>),
    );
    let stderr = drain(
        child
            .stderr
            .take()
            .map(|s| Box::new(s) as Box<dyn io::Read + Send>),
    );
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            let _ = (stdout.join(), stderr.join());
            return Ok(Outcome::Failed);
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let mut text = stdout.join().unwrap_or_default();
    text.push_str(&stderr.join().unwrap_or_default());
    let errors = compile_errors(&text);
    Ok(if status.success() && errors == 0 {
        Outcome::Compiled
    } else if errors > 0 || text.contains(": Error: ") {
        Outcome::Errors(text)
    } else {
        Outcome::Failed
    })
}

/// The error count `frontend_server` reports on its result line
/// (`<boundary key> <output dill> <errors>`); `0` for compilers that print
/// no such line (`dart compile kernel` signals errors by exit status).
/// An unparsable count counts as an error.
fn compile_errors(stdout: &str) -> usize {
    stdout
        .lines()
        .rev()
        .find_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            (fields.len() == 3 && fields[1].ends_with(".dill"))
                .then(|| fields[2].parse().unwrap_or(1))
        })
        .unwrap_or(0)
}

/// The entry point: every suite imported by a directory-relative URI, a
/// map from each suite's absolute path to its `main`, and the platform
/// plugin. Written for every null-safe language version (no records,
/// class modifiers or switch expressions), since it takes the package's
/// default language version.
#[must_use]
pub fn entry_point(out_dir: &Path, suites: &[PathBuf]) -> String {
    let mut src = String::with_capacity(4096 + suites.len() * 160);
    src.push_str(PRELUDE);
    for (index, suite) in suites.iter().enumerate() {
        let _ = writeln!(
            src,
            "import '{}' as s{index};",
            relative_uri(out_dir, suite)
        );
    }
    src.push_str(
        "\nfinal Map<String, Function Function()> _suites = <String, Function Function()>{\n",
    );
    for (index, suite) in suites.iter().enumerate() {
        let _ = writeln!(
            src,
            "  {}: () => s{index}.main,",
            dart_string(&suite.to_string_lossy())
        );
    }
    src.push_str("};\n");
    src.push_str(PLATFORM);
    src
}

const PRELUDE: &str = r"// Generated by dartq. Do not edit.
// ignore_for_file: implementation_imports, directives_ordering

import 'dart:async';
import 'dart:convert';
import 'dart:isolate';

import 'package:async/async.dart';
import 'package:path/path.dart' as p;
import 'package:stream_channel/isolate_channel.dart';
import 'package:stream_channel/stream_channel.dart';
import 'package:test_api/backend.dart';
import 'package:test_core/src/bootstrap/vm.dart';
import 'package:test_core/src/executable.dart' as executable;
import 'package:test_core/src/runner/configuration.dart';
import 'package:test_core/src/runner/hack_register_platform.dart';
import 'package:test_core/src/runner/platform.dart';
import 'package:test_core/src/runner/plugin/environment.dart';
import 'package:test_core/src/runner/plugin/platform_helpers.dart';
import 'package:test_core/src/runner/runner_suite.dart';
import 'package:test_core/src/runner/suite.dart';
import 'package:test_core/src/runner/vm/platform.dart';
import 'package:test_core/src/runner/configuration/reporters.dart';
import 'package:test_core/src/runner/load_suite.dart';
import 'package:test_core/src/runner/reporter/json.dart';

";

const PLATFORM: &str = r"
void _entry(List<Object?> message) => internalBootstrapVmTest(
    _suites[message[0] as String]!, message[1] as SendPort);

/// Starts each suite in a new isolate of this program's group; everything
/// else is the stock VM platform.
class _GroupPlatform extends PlatformPlugin {
  final VMPlatform _stock = VMPlatform();

  @override
  Future<RunnerSuite?> load(String path, SuitePlatform platform,
      SuiteConfiguration suiteConfig, Map<String, Object?> message) async {
    final key = p.canonicalize(p.absolute(path));
    final config = Configuration.current;
    if (platform.compiler != Compiler.kernel ||
        !_suites.containsKey(key) ||
        config.debug ||
        config.pauseAfterLoad) {
      return _stock.load(path, platform, suiteConfig, message);
    }
    final receivePort = ReceivePort();
    Isolate isolate;
    try {
      isolate = await Isolate.spawn(_entry, <Object?>[key, receivePort.sendPort],
          debugName: 'test_suite:$key');
    } catch (_) {
      receivePort.close();
      rethrow;
    }
    final outer =
        MultiChannel<Object?>(IsolateChannel<Object?>.connectReceive(receivePort));
    final queue = StreamQueue<Object?>(outer.stream);
    final id = (await queue.next) as int;
    final channel = outer.virtualChannel(id).transformStream(
          StreamTransformer<Object?, Object?>.fromHandlers(handleDone: (sink) {
            isolate.kill();
            outer.sink.close();
            sink.close();
          }),
        );
    final controller = deserializeSuite(path, platform, suiteConfig,
        const PluginEnvironment(), channel.cast<Object?>(), message);
    return controller.suite;
  }

  @override
  Future<void> close() => _stock.close();
}

Future<void> main(List<String> args) async {
  registerPlatformPlugin(<Runtime>[Runtime.vm], () => _GroupPlatform());
  // The stock JSON reporter, plus one marker line per suite once the suite
  // has closed (every one of its events is written by then): the caller
  // settles each suite as soon as it finishes instead of at the end of
  // the shard. Registered under `json` so every behavior keyed on that
  // reporter name (test location capture) stays stock.
  registerReporter(
      'json',
      ReporterDetails('A machine-readable format.', (config, engine, sink) {
        final reporter =
            JsonReporter.watch(engine, sink, isDebugRun: config.debug);
        engine.onSuiteStarted.listen((liveSuite) {
          final suite = liveSuite.suite;
          if (suite is LoadSuite) return;
          liveSuite.onClose.then((_) {
            sink.writeln(jsonEncode(<String, Object?>{
              'type': 'dartq.suiteDone',
              'path': suite.path,
            }));
          });
        });
        return reporter;
      }));
  await executable.main(args);
}
";

/// A Dart single-quoted string literal for `text`.
pub(crate) fn dart_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('\'');
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            '$' => out.push_str("\\$"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{{{:x}}}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('\'');
    out
}

/// `to` as a URI relative to the directory `from_dir`, each segment
/// percent-encoded except RFC 3986 unreserved characters, so any file name
/// survives as an import URI.
fn relative_uri(from_dir: &Path, to: &Path) -> String {
    use std::path::Component;
    let from: Vec<Component<'_>> = from_dir.components().collect();
    let target: Vec<Component<'_>> = to.components().collect();
    let common = from.iter().zip(&target).take_while(|(a, b)| a == b).count();
    let mut parts: Vec<String> = vec!["..".to_owned(); from.len() - common];
    for component in &target[common..] {
        parts.push(encode_segment(&component.as_os_str().to_string_lossy()));
    }
    parts.join("/")
}

fn encode_segment(segment: &str) -> String {
    let mut out = String::with_capacity(segment.len());
    for byte in segment.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(byte));
        } else {
            let _ = write!(out, "%{byte:02X}");
        }
    }
    out
}

/// Split `suites` into `shards` groups of near-equal predicted cost,
/// longest first (LPT): `cost` gives each suite's expected run time,
/// unknown suites cost the mean of the known ones. Shards run as separate
/// processes: one isolate group per process keeps garbage collection (which
/// pauses a whole group) from serializing every suite of the run.
#[must_use]
pub fn shard(
    suites: &[PathBuf],
    shards: usize,
    cost: impl Fn(&Path) -> Option<u64>,
) -> Vec<Vec<PathBuf>> {
    let shards = shards.clamp(1, suites.len().max(1));
    let known: Vec<u64> = suites.iter().filter_map(|suite| cost(suite)).collect();
    let fallback = if known.is_empty() {
        1
    } else {
        (known.iter().sum::<u64>() / known.len() as u64).max(1)
    };
    let mut weighted: Vec<(u64, &PathBuf)> = suites
        .iter()
        .map(|suite| (cost(suite).unwrap_or(fallback).max(1), suite))
        .collect();
    weighted.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));
    let mut bins: Vec<(u64, Vec<PathBuf>)> = vec![(0, Vec::new()); shards];
    for (weight, suite) in weighted {
        let bin = bins
            .iter_mut()
            .min_by_key(|bin| bin.0)
            .expect("at least one shard");
        bin.0 += weight;
        bin.1.push(suite.clone());
    }
    bins.into_iter()
        .map(|(_, suites)| suites)
        .filter(|suites| !suites.is_empty())
        .collect()
}

/// Recorded run time per suite of one package, for balancing shards. A
/// performance hint only — never part of any cache key or verdict — kept
/// beside the package's stamp index.
#[derive(Debug, Default)]
pub struct Durations {
    millis: std::collections::BTreeMap<String, u64>,
}

impl Durations {
    fn path(root: &Path) -> PathBuf {
        root.join(".dart_tool")
            .join("dartq")
            .join("test")
            .join("durations.json")
    }

    /// The durations recorded for the package at `root`; empty when none.
    #[must_use]
    pub fn load(root: &Path) -> Self {
        let millis = std::fs::read(Self::path(root))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self { millis }
    }

    /// The recorded run time of `suite` (absolute), if any.
    #[must_use]
    pub fn get(&self, root: &Path, suite: &Path) -> Option<u64> {
        let rel = suite.strip_prefix(root).ok()?;
        self.millis
            .get(&rel.to_string_lossy().replace('\\', "/"))
            .copied()
    }

    /// Record per-row durations (`(package-relative suite, ms)`), summed per
    /// suite; suites not in this run keep their earlier value. Best effort:
    /// a failed write only costs balance on the next run.
    pub fn record(mut self, root: &Path, rows: &[(PathBuf, u64)]) {
        let mut sums: std::collections::BTreeMap<String, u64> = std::collections::BTreeMap::new();
        for (file, ms) in rows {
            *sums
                .entry(file.to_string_lossy().replace('\\', "/"))
                .or_default() += ms;
        }
        if sums.is_empty() {
            return;
        }
        self.millis.extend(sums);
        if let Ok(bytes) = serde_json::to_vec(&self.millis) {
            let _ = crate::stamp::write_atomic(&Self::path(root), &bytes);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_point_maps_every_suite() {
        let out = Path::new("/ws/.dart_tool/dartq/group");
        let suites = [
            PathBuf::from("/ws/test/a b_test.dart"),
            PathBuf::from("/ws/test/it's$_test.dart"),
        ];
        let src = entry_point(out, &suites);
        assert!(src.contains("import '../../../test/a%20b_test.dart' as s0;"));
        assert!(src.contains("import '../../../test/it%27s%24_test.dart' as s1;"));
        assert!(src.contains("'/ws/test/a b_test.dart': () => s0.main,"));
        assert!(src.contains("'/ws/test/it\\'s\\$_test.dart': () => s1.main,"));
        assert!(src.contains("registerPlatformPlugin(<Runtime>[Runtime.vm]"));
    }

    /// Errors confined to suite files name those suites; an error in any
    /// other file, or an unreadable location, names none.
    #[test]
    fn broken_suites_only_when_every_error_is_in_a_suite() {
        let root = Path::new("/ws");
        let hosted = [
            PathBuf::from("/ws/test/a_test.dart"),
            PathBuf::from("/ws/test/b_test.dart"),
        ];
        let suite_error = "result k\n\
            test/a_test.dart:3:25: Error: A value of type 'String' can't be assigned.\n\
            void broken() { int x = \"s\"; }\n                        ^\n\
            test/a_test.dart:9:1: Error: Expected ';' after this.\n\
            k /ws/.dart_tool/dartq/group/group.next.dill 2\n";
        assert_eq!(
            broken_suites(root, &hosted, suite_error),
            Some(vec![hosted[0].clone()])
        );
        let absolute = "/ws/test/b_test.dart:1:1: Error: Not found: 'missing.dart'\n";
        assert_eq!(
            broken_suites(root, &hosted, absolute),
            Some(vec![hosted[1].clone()])
        );
        let lib_error =
            format!("{suite_error}lib/src/x.dart:100:20: Error: A value of type 'String'.\n");
        assert_eq!(
            broken_suites(root, &hosted, &lib_error),
            None,
            "a shared file breaks every suite"
        );
        assert_eq!(broken_suites(root, &hosted, "x: Error: oops\n"), None);
        assert_eq!(
            broken_suites(root, &hosted, "k out.dill 1\n"),
            None,
            "no location at all"
        );
    }

    #[test]
    fn frontend_server_result_lines() {
        assert_eq!(
            compile_errors("+file:///a.dart\nabc-key /x/group.next.dill 0\n"),
            0
        );
        assert_eq!(compile_errors("abc-key /x/group.next.dill 3\n"), 3);
        assert_eq!(compile_errors("abc-key /x/group.next.dill ?\n"), 1);
        assert_eq!(
            compile_errors(""),
            0,
            "dart compile kernel prints no result line"
        );
    }

    #[test]
    fn shards_balance_by_cost() {
        let suites: Vec<PathBuf> = (0..6)
            .map(|i| PathBuf::from(format!("/t/{i}_test.dart")))
            .collect();
        let costs = [90u64, 10, 10, 10, 40, 40];
        let cost = |path: &Path| {
            let index: usize = path
                .file_name()?
                .to_str()?
                .split('_')
                .next()?
                .parse()
                .ok()?;
            costs.get(index).copied()
        };
        let parts = shard(&suites, 2, cost);
        assert_eq!(parts.len(), 2);
        let load = |part: &Vec<PathBuf>| part.iter().map(|suite| cost(suite).unwrap()).sum::<u64>();
        assert_eq!(load(&parts[0]) + load(&parts[1]), 200);
        assert!(load(&parts[0]).abs_diff(load(&parts[1])) <= 20, "{parts:?}");
        // Every suite lands in exactly one shard; more shards than suites
        // collapse to one per suite.
        assert_eq!(shard(&suites, 64, cost).len(), 6);
        assert_eq!(shard(&suites, 0, |_| None).len(), 1);
    }

    /// Recorded durations load back per suite, summed across rows (the
    /// rows carry package-relative paths, as the runners record them);
    /// suites outside the package, or a corrupt index, key nothing.
    #[test]
    fn durations_round_trip_per_suite() {
        let tmp = tempfile::tempdir().expect("tmp");
        let root = tmp.path().canonicalize().expect("canon");
        let a = root.join("test/a_test.dart");
        let b = root.join("test/b_test.dart");
        assert_eq!(Durations::load(&root).get(&root, &a), None);
        let rel = |suite: &Path| PathBuf::from(suite.strip_prefix(&root).expect("inside"));
        Durations::load(&root).record(&root, &[(rel(&a), 100), (rel(&a), 50), (rel(&b), 200)]);
        let durations = Durations::load(&root);
        assert_eq!(durations.get(&root, &a), Some(150));
        assert_eq!(durations.get(&root, &b), Some(200));
        assert_eq!(durations.get(&root, &root.join("test/c_test.dart")), None);
        assert_eq!(
            durations.get(Path::new("/elsewhere"), &a),
            None,
            "outside the package"
        );
        // A corrupt index starts empty, and recording no rows writes nothing.
        std::fs::write(Durations::path(&root), b"{").expect("write");
        Durations::load(&root).record(&root, &[]);
        assert_eq!(Durations::load(&root).get(&root, &a), None);
    }

    /// The command line that starts a compiled runner: asserts on, the
    /// package configuration it was compiled against, then the kernel.
    #[test]
    fn runner_args_start_the_kernel() {
        let runner = GroupRunner {
            kernel: PathBuf::from("/k/group.dill"),
            packages: PathBuf::from("/p/.dart_tool/package_config.json"),
        };
        assert_eq!(
            runner.args(),
            [
                "--enable-asserts",
                "--packages=/p/.dart_tool/package_config.json",
                "/k/group.dill"
            ]
        );
    }
}
