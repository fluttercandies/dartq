//! The grouped pass of `dartq test --flutter`: every suite still to run
//! is compiled into a few shared programs and run in reused
//! `flutter_tester` processes under the stock `package:test` runner.
//!
//! ## Why
//!
//! `flutter test` compiles every suite into its own kernel through one
//! serial `frontend_server` and starts a fresh `flutter_tester` for it:
//! each suite pays a whole-program compile (~1 s for a widget test), a
//! process start, and a cold load and JIT of the framework. On a
//! 3000-suite package (10-core host) that is 1290 s.
//!
//! Here suites are compiled in chunks of [`CHUNK`] into one kernel each by
//! one resident `frontend_server` — the first chunk ~6 s, every later one
//! ~0.4 s, since each reuses the compiler's loaded framework — and each
//! tester hosts one chunk: it starts once, then spawns one engine per suite
//! (the engine's `Spawn`, the mechanism `--experimental-faster-testing`
//! uses), a new isolate in the chunk's group sharing its loaded and
//! JIT-compiled code but none of its Dart state.
//!
//! ## What stays stock
//!
//! The compile command and the tester command line and environment are
//! the tool's own, read from its trace of one run ([`Recipe::probe`]).
//! The runner is `package:test`'s own engine, from the `test_core` the tool
//! pins, with the tool's arguments; each suite engine runs the tool's
//! bootstrap (error listener, golden comparator rooted at the suite,
//! `flutter_test_config.dart` wrapping) and talks to the runner over the
//! same JSON-over-WebSocket protocol.
//!
//! ## What differs, and how the caller stays exact
//!
//! - Every engine of a tester shares the process: fonts, the isolate name
//!   server, native memory, the working directory. A suite reaching any of
//!   them ([`crate::deps::FileFacts::engine_shared`]) gets a tester of its
//!   own, retired after it.
//! - A suite whose own file carries `// dartq: shardable` may occupy two
//!   rows ([`Hosted::shard_total`]): two engines of its chunk run
//!   complementary selections of its top-level entries (the same mechanism
//!   the runner's own `--total-shards` uses, applied per row), and the
//!   caller merges the two blocks back into the suite's one.
//! - The tool prints whatever a tester writes to its standard streams
//!   (`Shell: …`, under `--machine` too). A suite is only reported clean
//!   when its tester wrote nothing but the runner's markers while the suite
//!   was in flight; a dirty suite reruns stock.
//! - `Platform.script` and `Platform.executableArguments` name the chunk,
//!   not the suite's listener: suites observing them run stock
//!   ([`crate::deps::FileFacts::launch`]).
//! - A chunk with a compile error outside its suites' own files compiles
//!   nothing; its suites run stock, as does any suite whose own file
//!   fails to compile.
//!
//! The caller treats the grouped pass as a fast first pass: suites that
//! are green and clean are final; every other suite reruns under stock
//! `flutter test`, which reports it exactly as it always does.

use std::fmt::Write as _;
use std::io::{self, BufRead, BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// Suites per compiled chunk. A chunk's kernel holds the framework once
/// plus its suites; one tester process hosts one chunk at a time. Measured
/// on a 3000-suite widget-test package: chunks of 300 compile in 0.4 s
/// each after the first and keep a tester at 0.24–0.40 GB; the whole
/// package in one kernel holds every tester at 0.44 GB for the same work.
pub const CHUNK: usize = 300;

/// The runner's fixed source, compiled once per toolchain.
const RUNNER: &str = include_str!("flutter/runner.dart");

/// The fixed part of every chunk's entry point.
const TESTER: &str = include_str!("flutter/tester.dart");

/// The suite [`Recipe::probe`] runs: it reports the environment the tool
/// gives a tester.
const PROBE: &str = "import 'dart:convert';\nimport 'dart:io';\n\n\
import 'package:flutter_test/flutter_test.dart';\n\n\
void main() => test('probe', () => print('dartq.environment ${jsonEncode(Platform.environment)}'));\n";

/// The probe's line carrying the tester environment.
const ENVIRONMENT: &str = "dartq.environment ";

/// How the tool compiles and starts a suite, read from its own trace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Recipe {
    /// `frontend_server` as the tool starts it, minus the per-run
    /// `--output-dill`, `--initialize-from-dill`, `--packages` and
    /// `--incremental` arguments, which dartq supplies.
    pub compiler: Vec<String>,
    /// The tool's shared test-compile seed: the `--initialize-from-dill`
    /// target its trace carried (`build/test_cache/build/<build-key>.cache
    /// .dill[.track.dill]`). Every `flutter test` process initializes its
    /// resident compiler from this file; writing a richer kernel here
    /// before the stock shards start seeds every shard's first compile.
    /// Empty when the trace carried no seed (or the recipe predates this
    /// field): no pre-seeding.
    #[serde(default)]
    pub test_cache: PathBuf,
    /// `flutter_tester` and its arguments, without the trailing kernel.
    pub tester: Vec<String>,
    /// The tester's whole environment as the probe suite observed it
    /// (the tool's own environment, `FLUTTER_ROOT` and its analytics
    /// variables included), minus `SERVER_PORT` (the runner's own) and
    /// `FONTCONFIG_FILE` (dartq writes the tool's fonts configuration).
    pub environment: Vec<(String, String)>,
    /// The tool's font configuration file contents.
    pub fonts: String,
}

/// The arguments of a traced command line. The trace prints commands
/// joined by single spaces; dartq refuses any recipe whose arguments could
/// not be split back unambiguously.
fn split_trace(line: &str) -> Option<Vec<String>> {
    let parts: Vec<String> = line.split(' ').map(str::to_owned).collect();
    (parts.iter().all(|part| !part.is_empty())).then_some(parts)
}

/// `[a, b, c]` as Dart's `List.toString` prints it; `None` when an element
/// itself contains `", "` (the split would be ambiguous).
fn split_list(text: &str) -> Option<Vec<String>> {
    let inner = text.strip_prefix('[')?.strip_suffix(']')?;
    if inner.is_empty() {
        return Some(Vec::new());
    }
    Some(inner.split(", ").map(str::to_owned).collect())
}

/// The trace text after the logger's `[ +N ms] ` prefix.
fn trace_body(line: &str) -> &str {
    match line
        .strip_prefix('[')
        .and_then(|rest| rest.split_once("] "))
    {
        Some((_, body)) => body,
        None => line,
    }
}

impl Recipe {
    /// Run the tool once, verbosely, on a generated empty suite under the
    /// package's `.dart_tool/dartq`, and read the compile and tester
    /// commands it traced. `args` are the tool arguments that shape either
    /// (`--dart-define`, `--enable-experiment`, `--flavor`, impeller, …).
    /// `None` when the tool's trace does not have the expected shape.
    ///
    /// # Errors
    ///
    /// I/O failures writing the probe or running the tool.
    pub fn probe(flutter: &Path, root: &Path, args: &[String]) -> io::Result<Option<Self>> {
        let dir = root.join(".dart_tool").join("dartq").join("flutter");
        std::fs::create_dir_all(&dir)?;
        let probe = dir.join("probe_test.dart");
        crate::stamp::write_atomic(&probe, PROBE.as_bytes())?;
        let output = Command::new(flutter)
            .args(["test", "-v", "--no-pub", "--reporter", "json"])
            .args(args)
            .arg(&probe)
            .current_dir(root)
            .stdin(Stdio::null())
            .output()?;
        if !output.status.success() {
            return Ok(None);
        }
        Ok(Self::parse(&String::from_utf8_lossy(&output.stdout)))
    }

    /// The recipe in a tool trace (see [`Self::probe`]).
    #[must_use]
    pub fn parse(trace: &str) -> Option<Self> {
        let mut compiler = None;
        let mut tester = None;
        let mut environment = None;
        for line in trace.lines() {
            if let Some(event) = crate::event::parse(line) {
                if let Some(text) = event
                    .str_at(&["message"])
                    .and_then(|message| message.strip_prefix(ENVIRONMENT))
                {
                    environment = Some(
                        serde_json::from_str::<std::collections::BTreeMap<String, String>>(text)
                            .ok()?,
                    );
                }
                continue;
            }
            let body = trace_body(line);
            if compiler.is_none()
                && body.contains("frontend_server")
                && body.contains(" --sdk-root ")
            {
                compiler = Some(split_trace(body)?);
            } else if let Some(rest) =
                body.strip_prefix("test 0: Starting flutter_tester process with command=")
            {
                let (command, _) = rest.split_once("], environment=")?;
                let command = split_list(&format!("{command}]"))?;
                // Every argument after the binary is a flag, but the last
                // (the kernel): anything else means the split was ambiguous.
                if command.len() < 2
                    || command[1..command.len() - 1]
                        .iter()
                        .any(|arg| !arg.starts_with("--"))
                {
                    return None;
                }
                tester = Some(command);
            }
        }
        let compiler = compiler?;
        let mut command = tester?;
        let environment = environment?;
        // The kernel is the last argument: the listener the tool compiled.
        command.pop()?;
        // Per-run arguments dartq supplies itself — except
        // `--initialize-from-dill`'s value, the tool's shared test-compile
        // seed location, which dartq keeps to pre-seed the stock shards.
        let mut kept = Vec::with_capacity(compiler.len());
        let mut test_cache = None;
        let mut parts = compiler.into_iter();
        while let Some(part) = parts.next() {
            match part.as_str() {
                "--output-dill" | "--packages" => {
                    parts.next()?;
                }
                "--initialize-from-dill" => {
                    test_cache = parts.next().map(PathBuf::from);
                }
                "--incremental" => {}
                _ => kept.push(part),
            }
        }
        let test_cache = test_cache.unwrap_or_default();
        let environment: Vec<(String, String)> = environment
            .into_iter()
            .filter(|(key, _)| key != "SERVER_PORT" && key != "FONTCONFIG_FILE")
            .collect();
        // The tool deletes its fonts configuration on exit; the contents are
        // fixed (`FontConfigManager`): the SDK's artifact cache directory.
        let artifacts = Path::new(kept.first()?)
            .ancestors()
            .find(|dir| dir.ends_with("bin/cache"))?
            .join("artifacts");
        let fonts = format!(
            "<fontconfig>\n  <dir>{}</dir>\n  <cachedir>/var/cache/fontconfig</cachedir>\n</fontconfig>\n",
            artifacts.display()
        );
        Some(Self {
            compiler: kept,
            test_cache,
            tester: command,
            environment,
            fonts,
        })
    }
}

/// One suite of the grouped pass.
#[derive(Debug, Clone)]
pub struct Hosted {
    /// The absolute path the runner reports the suite under.
    pub path: PathBuf,
    /// The nearest `flutter_test_config.dart`, as the tool finds it.
    pub config: Option<PathBuf>,
    /// Runs alone in a tester of its own.
    pub fresh: bool,
    /// A deterministic digest of the suite's import-closure bytes plus the
    /// toolchain stamps ([`closure_digests`]). `None` only when the package
    /// configuration could not be read — the chunk key then falls back to
    /// the suite's own file bytes.
    pub digest: Option<String>,
    /// How many rows the suite occupies in the pass: `1` runs it whole;
    /// `2` splits its top-level entries into two complementary selections
    /// run as two engines of its chunk (opt-in — the suite's own file must
    /// carry [`crate::deps::SHARDABLE_MARKER`], and the caller decides by
    /// the gate: long enough, workers available, no user sharding).
    ///
    /// A suite with a `flutter_test_config` is never sharded by the caller:
    /// the config's `testExecutable` wraps the suite's `main` *outside* the
    /// shard filter, so both shards would run the wrapper's own
    /// `setUpAll`-level effects twice with no complementary benefit, and a
    /// config that adds tests of its own would report them twice.
    pub shard_total: u32,
}

/// The entry point of one chunk: the fixed tester part, the chunk's
/// imports, and one entry point per suite wiring the suite's `main` the way
/// the stock bootstrap does (`_testMain`, wrapped by the nearest
/// `flutter_test_config.dart`'s `testExecutable`). The entry takes the
/// package's language version; the suites keep their own.
#[must_use]
pub fn chunk_entry(index: usize, suites: &[(usize, &Hosted)]) -> String {
    let mut src = String::with_capacity(TESTER.len() + suites.len() * 400);
    let mut configs: Vec<&Path> = Vec::new();
    let mut header = String::new();
    for &(i, suite) in suites {
        let _ = writeln!(
            header,
            "import {} as s{i};",
            crate::group::dart_string(&file_uri(&suite.path))
        );
        if let Some(config) = &suite.config {
            if !configs.contains(&config.as_path()) {
                let _ = writeln!(
                    header,
                    "import {} as c{};",
                    crate::group::dart_string(&file_uri(config)),
                    configs.len()
                );
                configs.push(config);
            }
        }
    }
    src.push_str(&TESTER.replacen("// dartq:suites\n", &header, 1));
    let _ = writeln!(src, "\nconst int _chunk = {index};");
    for &(i, suite) in suites {
        let url = crate::group::dart_string(&file_uri(&suite.path));
        let main = match &suite.config {
            Some(config) => {
                let c = configs
                    .iter()
                    .position(|seen| *seen == config.as_path())
                    .expect("imported above");
                format!("() => () => c{c}.testExecutable(() async {{ await Future(s{i}.main); }})")
            }
            None => format!("() => () async {{ await Future(s{i}.main); }}"),
        };
        let _ = writeln!(
            src,
            "\n@pragma('vm:entry-point')\nvoid _s{i}() => _connect({i}, {url}, {main});"
        );
    }
    src
}

/// `Uri.file(path)` for an absolute path, as the tool writes suite URLs.
fn file_uri(path: &Path) -> String {
    let mut out = String::from("file://");
    for component in path.components() {
        if let std::path::Component::Normal(part) = component {
            out.push('/');
            out.push_str(&crate::cache::encode_uri_segment(&part.to_string_lossy()));
        }
    }
    out
}

/// The content identity of one chunk's kernel: everything the compiler
/// reads to emit it. A change to any input re-keys the chunk to a fresh
/// compile; only an exact match adopts the previous run's kernel.
///
/// Inputs, per chunk:
/// - the generated entry source (every suite's URI, its
///   `flutter_test_config` wiring, the membership set and the chunk index);
/// - the resolved `package_config.json` the compiler reads;
/// - the recipe's compiler command line;
/// - **each hosted suite's run-cache key**, which already folds that
///   suite's whole import-closure content digests, its
///   `flutter_test_config.dart` closure bytes and the toolchain stamp —
///   folding it verbatim covers a source/lib/dependency/config edit
///   without a second hashing pass over the closure. A suite with no
///   cache key (a refused key) falls back to its own file bytes plus its
///   config's bytes, a conservative floor — any doubt re-keys.
///
/// `hosted` is the local-index subset actually compiled (a failed suite is
/// retried out, so the key follows the reduced set, not the original).
fn chunk_key(
    plan: &Plan<'_>,
    index: usize,
    hosted: &[usize],
    member_of: impl Fn(usize) -> usize,
) -> String {
    let suites: Vec<(usize, &Hosted)> = hosted
        .iter()
        .map(|&local| (local, &plan.suites[member_of(local)]))
        .collect();
    let entry = chunk_entry(index, &suites);
    let mut hasher = blake3::Hasher::new();
    hasher.update(entry.as_bytes());
    hasher.update(&std::fs::read(plan.packages).unwrap_or_default());
    for arg in &plan.recipe.compiler {
        hasher.update(&(arg.len() as u64).to_le_bytes());
        hasher.update(arg.as_bytes());
    }
    for (_, suite) in &suites {
        // Some: the deterministic closure digest carries this suite's
        // reachable-file bytes, its config's bytes and the toolchain
        // stamps.
        if let Some(digest) = &suite.digest {
            hasher.update(digest.as_bytes());
            continue;
        }
        // None: no closure to lean on — key the chunk on the suite's and
        // its config's own bytes so a content edit still re-keys.
        hasher.update(&std::fs::read(&suite.path).unwrap_or_default());
        if let Some(config) = &suite.config {
            hasher.update(&std::fs::read(config).unwrap_or_default());
        }
    }
    hasher.finalize().to_hex().to_string()
}

/// The `.key` a fresh chunk *wants* (full member set), and the member
/// lookup for it — adoption compares this against the stored `.key`.
/// `chunks` hold row indices; `rows` maps a row to its suite.
fn chunk_keys(plan: &Plan<'_>, rows: &[Row], chunks: &[Vec<usize>]) -> Vec<String> {
    chunks
        .iter()
        .enumerate()
        .map(|(index, members)| {
            let locals: Vec<usize> = (0..members.len()).collect();
            chunk_key(plan, index, &locals, |local| rows[members[local]].suite)
        })
        .collect()
}

/// One resident `frontend_server`, compiling chunk after chunk: the first
/// compile loads the framework, every later one reuses it (`accept` then
/// `reset`, as the tool's own test compiler does, so each output is a
/// whole program).
struct Compiler {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<std::process::ChildStdout>,
    output: PathBuf,
    first: bool,
}

/// How one chunk compile ended.
enum Compiled {
    /// The kernel at the compiler's output path.
    Kernel,
    /// Compile errors; the compiler's output.
    Errors(String),
}

impl Compiler {
    fn start(
        recipe: &Recipe,
        packages: &Path,
        output: &Path,
        root: &Path,
        seed: Option<&Path>,
    ) -> io::Result<Self> {
        let (program, args) = recipe
            .compiler
            .split_first()
            .ok_or_else(|| io::Error::other("empty compiler command"))?;
        let mut cmd = Command::new(program);
        cmd.args(args)
            .arg("--incremental")
            .arg("--packages")
            .arg(packages)
            .arg("--output-dill")
            .arg(output);
        // The tool's own test compiler seeds each run's first compile from
        // the previous run's kernel (`--initialize-from-dill`): the
        // incremental compiler byte-compares every recorded URI against the
        // filesystem and invalidates what changed, so a stale seed never
        // shapes the output — it only skips recompiling what did not
        // change (the whole framework, in practice).
        if let Some(seed) = seed.filter(|seed| seed.is_file()) {
            cmd.arg("--initialize-from-dill").arg(seed);
        }
        let mut child = cmd
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let stdin = child.stdin.take().expect("piped");
        let stdout = BufReader::new(child.stdout.take().expect("piped"));
        Ok(Self {
            child,
            stdin,
            stdout,
            output: output.to_path_buf(),
            first: true,
        })
    }

    /// Compile `entry` (a file path) into the output path.
    fn compile(&mut self, entry: &Path) -> io::Result<Compiled> {
        let uri = file_uri(entry);
        if self.first {
            writeln!(self.stdin, "compile {uri}")?;
            self.first = false;
        } else {
            let key = format!("dartq-{}", blake3::hash(uri.as_bytes()).to_hex());
            writeln!(self.stdin, "recompile {uri} {key}\n{uri}\n{key}")?;
        }
        self.stdin.flush()?;
        let mut text = String::new();
        let mut boundary: Option<String> = None;
        let mut line = String::new();
        loop {
            line.clear();
            if self.stdout.read_line(&mut line)? == 0 {
                return Err(io::Error::other("frontend_server exited"));
            }
            let trimmed = line.trim_end_matches('\n');
            match &boundary {
                None => {
                    if let Some(key) = trimmed.strip_prefix("result ") {
                        boundary = Some(key.to_owned());
                    }
                }
                Some(key) => {
                    if let Some(rest) = trimmed.strip_prefix(key.as_str()) {
                        let rest = rest.trim();
                        if rest.is_empty() {
                            // The opening boundary (sources follow).
                            continue;
                        }
                        let errors: usize = rest
                            .rsplit(' ')
                            .next()
                            .and_then(|n| n.parse().ok())
                            .unwrap_or(1);
                        writeln!(self.stdin, "accept\nreset")?;
                        self.stdin.flush()?;
                        return Ok(if errors == 0 && self.output.is_file() {
                            Compiled::Kernel
                        } else {
                            Compiled::Errors(text)
                        });
                    }
                    if !trimmed.starts_with('+') && !trimmed.starts_with('-') {
                        text.push_str(trimmed);
                        text.push('\n');
                    }
                }
            }
        }
    }
}

impl Drop for Compiler {
    fn drop(&mut self) {
        let _ = writeln!(self.stdin, "quit");
        let _ = self.stdin.flush();
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Where the grouped pass keeps its files: per checkout, so concurrent
/// worktrees never share a kernel being written.
fn work_dir(root: &Path) -> PathBuf {
    root.join(".dart_tool").join("dartq").join("flutter")
}

/// The runner's kernel for `vm` (the tool's Dart VM) and the tool's own
/// package configuration (which pins `test_core`), compiled once and kept
/// under the user's dartq cache keyed on both.
///
/// # Errors
///
/// I/O failures, or a runner that does not compile.
pub fn runner_kernel(vm: &Path, tool_packages: &Path, scratch: &Path) -> io::Result<PathBuf> {
    let mut hasher = blake3::Hasher::new();
    hasher.update(RUNNER.as_bytes());
    hasher.update(vm.to_string_lossy().as_bytes());
    hasher.update(&std::fs::read(tool_packages)?);
    let key = hasher.finalize().to_hex();
    let dir = crate::host::cache_dir().map_or_else(
        || scratch.to_path_buf(),
        |dir| dir.join("test").join("flutter"),
    );
    let kernel = dir.join(format!("runner-{}.dill", &key[..32]));
    if kernel.is_file() {
        return Ok(kernel);
    }
    std::fs::create_dir_all(&dir)?;
    let source = dir.join(format!("runner-{}.dart", &key[..32]));
    crate::stamp::write_atomic(&source, RUNNER.as_bytes())?;
    let staged = kernel.with_extension(format!("dill.{}", std::process::id()));
    let output = Command::new(vm)
        .args(["compile", "kernel", "--verbosity=error"])
        .arg(format!("--packages={}", tool_packages.display()))
        .arg("-o")
        .arg(&staged)
        .arg(&source)
        .stdin(Stdio::null())
        .output()?;
    if !output.status.success() || !staged.is_file() {
        let _ = std::fs::remove_file(&staged);
        return Err(io::Error::other(format!(
            "the grouped runner did not compile: {}",
            String::from_utf8_lossy(&output.stdout)
        )));
    }
    std::fs::rename(&staged, &kernel)?;
    Ok(kernel)
}

/// Everything the grouped pass needs from its caller.
#[derive(Debug)]
pub struct Plan<'a> {
    pub root: &'a Path,
    /// The tool's Dart VM (`bin/cache/dart-sdk/bin/dart` of the Flutter SDK).
    pub vm: &'a Path,
    /// The tool's package configuration (`packages/flutter_tools`).
    pub tool_packages: &'a Path,
    /// The project's package configuration.
    pub packages: &'a Path,
    pub recipe: &'a Recipe,
    pub suites: &'a [Hosted],
    /// The `package:test` arguments the tool would pass (`--no-color`,
    /// filters, `--chain-stack-traces`, …), before `--`.
    pub runner_args: &'a [String],
    /// Concurrent suites (testers).
    pub concurrency: usize,
}

/// What the grouped pass reported.
#[derive(Debug)]
pub struct Outcome {
    /// The runner's event stream, as [`crate::execute`] parses it.
    pub execution: crate::Execution,
    /// Suites dropped before the run: their own file failed to compile, or
    /// their chunk did not compile at all.
    pub excluded: Vec<PathBuf>,
}

/// One row of the grouped pass: the `(suite, shard)` a row of `run.json`
/// describes. An unsharded suite has exactly one row (`shard == 0`); a
/// sharded one has [`Hosted::shard_total`] rows whose engines run
/// complementary selections of the suite's top-level entries.
#[derive(Debug, Clone, Copy)]
struct Row {
    /// Index into `plan.suites`.
    suite: usize,
    /// This row's shard index, `< plan.suites[suite].shard_total`.
    shard: u32,
}

/// The suite path a shard's run.json row and argv entry spell: the real
/// path for shard 0; for shard `i` the real path with `i` `/./` components
/// injected before the file name — a distinct map key for the runner's
/// `testSelections` (its entries collapse identical spellings) that still
/// stats and reads the real file, so `@Tags`/`@TestOn` metadata parse and
/// no shim file is needed.
fn shard_spelling(path: &Path, shard: u32) -> String {
    if shard == 0 {
        return path.display().to_string();
    }
    let mut out = String::with_capacity(path.as_os_str().len() + 2 * shard as usize);
    if let Some(parent) = path.parent() {
        out.push_str(&parent.display().to_string());
        out.push('/');
    }
    for _ in 0..shard {
        out.push_str("./");
    }
    out.push_str(&path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    ));
    out
}

/// Run `plan.suites` through the grouped pass: chunks compile in the
/// background, one after another, while the runner already runs the
/// suites of the chunks done; each suite's block reaches `cfg.sink` as the
/// suite settles.
///
/// # Errors
///
/// I/O failures writing the work files or starting the processes.
pub fn execute(plan: &Plan<'_>, cfg: &crate::ExecConfig) -> Result<Outcome, crate::ExecError> {
    let dir = work_dir(plan.root);
    std::fs::create_dir_all(&dir)?;
    let runner = runner_kernel(plan.vm, plan.tool_packages, &dir)?;
    // Chunk membership and each member's `local` (its `_s{local}`
    // entrypoint label) are canonical — members sorted by suite path —
    // so the chunk's compiled kernel is a pure function of *which* suites
    // it holds and their code, never of dispatch order. A run that sorts
    // suites differently for scheduling (longest-first) still wants the
    // same kernel and adopts it.
    let mut indices: Vec<usize> = (0..plan.suites.len()).collect();
    indices.sort_by(|&a, &b| plan.suites[a].path.cmp(&plan.suites[b].path));
    // Chunk membership counts *rows*, not suites: a sharded suite occupies
    // `shard_total` consecutive rows (each its own `_s{local}` entrypoint
    // and `testSelections` key) of one chunk — both shards' engines load
    // the same compiled kernel.
    let mut rows: Vec<Row> = Vec::new();
    for &suite in &indices {
        for shard in 0..plan.suites[suite].shard_total {
            rows.push(Row { suite, shard });
        }
    }
    let chunks: Vec<Vec<usize>> = (0..rows.len())
        .collect::<Vec<usize>>()
        .chunks(CHUNK)
        .map(<[usize]>::to_vec)
        .collect();
    let kernels: Vec<PathBuf> = (0..chunks.len())
        .map(|index| dir.join(format!("chunk{index}.dill")))
        .collect();
    // Chunk kernels persist across runs, keyed on the content they were
    // compiled from: a run whose chunk source is unchanged adopts the
    // previous run's kernel and skips the ~5 s framework-load compile
    // entirely (the store's `.key` sidecar names the inputs the compiler
    // saw). Any doubt re-keys to a compile — serving a stale kernel is the
    // one unrecoverable sin of this pass.
    //
    // A kernel whose `.key` no longer matches is kept as `chunkN.seed.dill`:
    // the incremental compiler initialized from it skips every unchanged
    // library (the framework is the bulk of a cold compile) while still
    // re-checking every source byte, so seeding is a pure speedup.
    let wanted_keys = chunk_keys(plan, &rows, &chunks);
    for (index, kernel) in kernels.iter().enumerate() {
        let adopted = kernel.is_file()
            && std::fs::read_to_string(kernel.with_extension("dill.key"))
                .ok()
                .is_some_and(|stored| stored.trim() == wanted_keys[index]);
        if adopted {
            // The sidecar was already written whole when the kernel landed;
            // only the hosted-suite list may still be missing (a run killed
            // between the two writes) — rewrite it idempotently.
            let locals: Vec<usize> = (0..chunks[index].len()).collect();
            crate::stamp::write_atomic(
                &kernel.with_extension("dill.json"),
                serde_json::to_string(&locals)
                    .map_err(io::Error::other)?
                    .as_bytes(),
            )?;
            let _ = std::fs::remove_file(kernel.with_extension("seed.dill"));
            continue;
        }
        if kernel.is_file() {
            let _ = std::fs::rename(kernel, kernel.with_extension("seed.dill"));
        }
        for stale in [
            kernel.with_extension("dill.failed"),
            kernel.with_extension("dill.json"),
            kernel.with_extension("dill.key"),
        ] {
            let _ = std::fs::remove_file(stale);
        }
    }
    let fonts = dir.join("fonts.conf");
    crate::stamp::write_atomic(&fonts, plan.recipe.fonts.as_bytes())?;
    let mut environment: serde_json::Map<String, serde_json::Value> = plan
        .recipe
        .environment
        .iter()
        .map(|(key, value)| (key.clone(), value.clone().into()))
        .collect();
    environment.insert(
        "FONTCONFIG_FILE".to_owned(),
        fonts.display().to_string().into(),
    );
    let mut run_rows = Vec::with_capacity(rows.len());
    for (chunk, members) in chunks.iter().enumerate() {
        for (local, &row) in members.iter().enumerate() {
            let Row { suite, shard } = rows[row];
            let suite = &plan.suites[suite];
            run_rows.push(serde_json::json!([
                shard_spelling(&suite.path, shard),
                chunk,
                local,
                suite.fresh,
                shard,
                suite.shard_total,
            ]));
        }
    }
    let mut args: Vec<String> = plan.runner_args.to_vec();
    // The engine's run pool must match the tester pool: `runner_args` never
    // carries `--concurrency` (the tool's `-j` drives its own scheduler,
    // not `testArgs`), and test_core's default is half the cores — which
    // would cap suites in flight below `plan.concurrency` testers.
    args.extend([
        "--concurrency".to_owned(),
        plan.concurrency.max(1).to_string(),
        "--reporter".to_owned(),
        "json".to_owned(),
        "--".to_owned(),
    ]);
    // One argv entry per row: a sharded suite's two rows stay adjacent, so
    // its shard engines co-start on two testers.
    for suite in plan.suites {
        for shard in 0..suite.shard_total {
            args.push(shard_spelling(&suite.path, shard));
        }
    }
    let config = serde_json::json!({
        "tester": plan.recipe.tester,
        "environment": environment,
        "chunks": kernels.iter().map(|kernel| kernel.display().to_string()).collect::<Vec<_>>(),
        "concurrency": plan.concurrency.max(1),
        "suites": run_rows,
        "args": args,
    });
    let config_path = dir.join("run.json");
    crate::stamp::write_atomic(&config_path, config.to_string().as_bytes())?;

    let excluded = std::sync::Mutex::new(Vec::new());
    let stop = std::sync::atomic::AtomicBool::new(false);
    let result = std::thread::scope(|scope| {
        let compiling =
            scope.spawn(|| compile_chunks(plan, &rows, &dir, &chunks, &kernels, &excluded, &stop));
        let mut cmd = Command::new(plan.vm);
        cmd.arg("--disable-dart-dev")
            .arg(&runner)
            .arg(&config_path)
            .current_dir(plan.root)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let ran = crate::exec::run_command_for(cmd, cfg);
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let compiled = compiling
            .join()
            .unwrap_or_else(|_| Err(io::Error::other("the chunk compiler panicked")));
        (ran, compiled)
    });
    let (ran, compiled) = result;
    compiled?;
    Ok(Outcome {
        execution: ran?,
        excluded: excluded.into_inner().unwrap_or_default(),
    })
}

/// Compile every chunk in order through one resident compiler, writing
/// each kernel (then its `.json` list of hosted locals) once whole, or a
/// `.failed` marker. A suite whose own file fails is dropped and its chunk
/// recompiled once without it; its rows in the runner stay, and loading
/// them fails, which the caller reruns stock.
fn compile_chunks(
    plan: &Plan<'_>,
    rows: &[Row],
    dir: &Path,
    chunks: &[Vec<usize>],
    kernels: &[PathBuf],
    excluded: &std::sync::Mutex<Vec<PathBuf>>,
    stop: &std::sync::atomic::AtomicBool,
) -> io::Result<()> {
    let output = dir.join("compile.dill");
    let mut compiler: Option<Compiler> = None;
    // `members` are row indices; a suite index resolves through `rows`.
    let suite_of = |local: usize, members: &[usize]| rows[members[local]].suite;
    for (index, members) in chunks.iter().enumerate() {
        if stop.load(std::sync::atomic::Ordering::Relaxed) {
            break;
        }
        // An adopted kernel needs no compiler at all — its `.key` matched,
        // `.json` is already in place, and the runner unblocks on the file.
        if kernels[index].is_file() {
            continue;
        }
        let mut hosted: Vec<usize> = (0..members.len()).collect();
        let mut retried = false;
        loop {
            let suites: Vec<(usize, &Hosted)> = hosted
                .iter()
                .map(|&local| (local, &plan.suites[suite_of(local, members)]))
                .collect();
            let entry = dir.join(format!("chunk{index}.dart"));
            crate::stamp::write_atomic(&entry, chunk_entry(index, &suites).as_bytes())?;
            if compiler.is_none() {
                compiler = Some(Compiler::start(
                    plan.recipe,
                    plan.packages,
                    &output,
                    plan.root,
                    Some(&kernels[index].with_extension("seed.dill")),
                )?);
            }
            let _ = std::fs::remove_file(&output);
            let result = match compiler.as_mut().expect("started").compile(&entry) {
                Ok(result) => result,
                Err(_) => {
                    // A crashed compiler: this chunk fails; the next one
                    // starts a fresh compiler.
                    compiler = None;
                    Compiled::Errors(String::new())
                }
            };
            match result {
                Compiled::Kernel => {
                    // Order matters: `.key` lands after the kernel rename so
                    // an interrupted run never leaves a key claiming a kernel
                    // that isn't there (adoption requires both).
                    std::fs::rename(&output, &kernels[index])?;
                    // The new kernel replaces the seed it was compiled from.
                    let _ = std::fs::remove_file(kernels[index].with_extension("seed.dill"));
                    // Key the kernel on what was actually compiled — a
                    // retried chunk's reduced set, so a later run wanting
                    // the full set re-keys and recompiles instead of
                    // adopting a kernel missing the excluded suite.
                    crate::stamp::write_atomic(
                        &kernels[index].with_extension("dill.key"),
                        chunk_key(plan, index, &hosted, |local| suite_of(local, members))
                            .as_bytes(),
                    )?;
                    crate::stamp::write_atomic(
                        &kernels[index].with_extension("dill.json"),
                        serde_json::to_string(&hosted)?.as_bytes(),
                    )?;
                    break;
                }
                Compiled::Errors(text) if !retried => {
                    let paths: Vec<PathBuf> = hosted
                        .iter()
                        .map(|&local| plan.suites[suite_of(local, members)].path.clone())
                        .collect();
                    let broken = crate::group::broken_suites(plan.root, &paths, &text);
                    match broken.filter(|broken| broken.len() < paths.len()) {
                        Some(broken) => {
                            hosted.retain(|&local| {
                                !broken.contains(&plan.suites[suite_of(local, members)].path)
                            });
                            excluded
                                .lock()
                                .map_err(|_| io::Error::other("poisoned"))?
                                .extend(broken);
                            retried = true;
                        }
                        None => {
                            fail_chunk(plan, &hosted, &kernels[index], excluded, |local| {
                                suite_of(local, members)
                            })?;
                            break;
                        }
                    }
                }
                Compiled::Errors(_) => {
                    fail_chunk(plan, &hosted, &kernels[index], excluded, |local| {
                        suite_of(local, members)
                    })?;
                    break;
                }
            }
        }
    }
    Ok(())
}

fn fail_chunk(
    plan: &Plan<'_>,
    hosted: &[usize],
    kernel: &Path,
    excluded: &std::sync::Mutex<Vec<PathBuf>>,
    suite_of: impl Fn(usize) -> usize,
) -> io::Result<()> {
    excluded
        .lock()
        .map_err(|_| io::Error::other("poisoned"))?
        .extend(
            hosted
                .iter()
                .map(|&local| plan.suites[suite_of(local)].path.clone()),
        );
    crate::stamp::write_atomic(&kernel.with_extension("dill.failed"), b"")
}

/// Seed the stock tool's shared test-compile cache (`recipe.test_cache`,
/// the `--initialize-from-dill` target every `flutter test` process
/// initializes its resident compiler from) with the largest chunk kernel
/// this checkout already holds. A `flutter test` shard's *first* compile
/// pays the framework-load unless its seed already carries it; our chunk
/// kernels carry exactly that — framework plus the suites' shared closure.
///
/// The tool keeps the largest kernel under this path (its own rule:
/// `firstCompile || !exists || cache.length < output.length`), so a bigger
/// chunk kernel replaces a smaller seed and is never itself displaced. The
/// incremental compiler byte-compares every URI recorded in the seed
/// against the filesystem before trusting it — a stale or foreign kernel
/// only loses speed, never correctness.
///
/// Best effort: any I/O failure just costs the speedup.
pub fn seed_test_cache(recipe: &Recipe, root: &Path) {
    let target = &recipe.test_cache;
    if target.as_os_str().is_empty() {
        return;
    }
    let dir = work_dir(root);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    let largest = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_name()?.to_str()?;
            (name.starts_with("chunk")
                && name.ends_with(".dill")
                && !name.ends_with(".seed.dill"))
            .then(|| (entry.metadata().map_or(0, |meta| meta.len()), path))
        })
        .max_by_key(|(size, _)| *size);
    let Some((size, kernel)) = largest else {
        return;
    };
    if std::fs::metadata(target).is_ok_and(|meta| meta.len() >= size) {
        return;
    }
    if let Some(parent) = target.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
    }
    // Copy through a sibling temp file so a shard never reads a
    // partially-written seed.
    let staged = target.with_extension(format!("seed-{}", std::process::id()));
    if std::fs::copy(&kernel, &staged).is_ok() {
        let _ = std::fs::rename(&staged, target).or_else(|_| {
            let _ = std::fs::remove_file(&staged);
            std::fs::copy(&kernel, target).map(|_| ())
        });
    } else {
        let _ = std::fs::remove_file(&staged);
    }
}

/// Where a suite runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    /// In a tester shared with other suites of its chunk.
    Shared,
    /// In a tester of its own ([`crate::deps::FileFacts::engine_shared`]).
    Fresh,
    /// Under the stock tool: it observes how its process was launched
    /// ([`crate::deps::FileFacts::launch`]), or its import closure cannot
    /// be read with certainty.
    Stock,
}

/// Where a suite runs and whether its author opted it into intra-suite
/// sharding ([`crate::deps::FileFacts::shardable`], read from the suite's
/// own file only — a marker inside a helper means nothing).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hosting {
    pub placement: Placement,
    /// The suite file carries `// dartq: shardable`.
    pub shardable: bool,
}

/// Place every suite (absolute paths) by its import closure. The Flutter
/// SDK's own packages are the framework every suite runs on either way
/// and are not entered; the package's own code and its hosted
/// dependencies are.
///
/// # Errors
///
/// I/O failures reading the package configuration.
pub fn place(root: &Path, suites: &[PathBuf]) -> io::Result<Vec<Hosting>> {
    use rayon::prelude::*;
    use rustc_hash::FxHashMap;

    let config_path = crate::cache::package_config_path(root);
    let text = std::fs::read_to_string(&config_path)?;
    let stock = || {
        vec![
            Hosting {
                placement: Placement::Stock,
                shardable: false,
            };
            suites.len()
        ]
    };
    let Ok(config) = dartq_pub::PackageConfig::parse(&text) else {
        return Ok(stock());
    };
    let Some(dart_tool) = config_path.parent() else {
        return Ok(stock());
    };
    let Ok(packages) = config.resolve(dart_tool) else {
        return Ok(stock());
    };
    let lib_dirs: FxHashMap<&str, &Path> = packages
        .iter()
        .map(|package| (package.name.as_str(), package.lib_dir.as_path()))
        .collect();
    let framework: Option<PathBuf> = config
        .additional_properties
        .get("flutterRoot")
        .and_then(serde_json::Value::as_str)
        .and_then(|uri| uri.strip_prefix("file://"))
        .map(PathBuf::from);
    let in_framework = |path: &Path| {
        framework
            .as_deref()
            .is_some_and(|sdk| path.starts_with(sdk))
    };

    // Breadth-first over the closure, one frontier read in parallel.
    #[derive(Default)]
    struct Node {
        deps: Vec<u32>,
        launch: bool,
        shared: bool,
        untrusted: bool,
        shardable: bool,
    }
    let mut ids: FxHashMap<PathBuf, u32> = FxHashMap::default();
    let mut nodes: Vec<Node> = Vec::new();
    let mut frontier: Vec<PathBuf> = Vec::new();
    for suite in suites {
        if !ids.contains_key(suite) {
            ids.insert(
                suite.clone(),
                u32::try_from(nodes.len()).unwrap_or(u32::MAX),
            );
            nodes.push(Node::default());
            frontier.push(suite.clone());
        }
    }
    while !frontier.is_empty() {
        let read: Vec<(
            PathBuf,
            Option<(crate::deps::FileFacts, Option<Vec<PathBuf>>)>,
        )> = frontier
            .par_drain(..)
            .map(|path| {
                let Ok(bytes) = std::fs::read(&path) else {
                    // A missing import is a compile error the run reports.
                    return (path, None);
                };
                let facts = crate::deps::facts(&bytes);
                let deps: Option<Vec<PathBuf>> = facts
                    .uris
                    .iter()
                    .map(|uri| crate::cache::resolve_uri(&path, uri, &lib_dirs))
                    .collect::<Option<Vec<Option<PathBuf>>>>()
                    .map(|deps| {
                        deps.into_iter()
                            .flatten()
                            .filter(|dep| !in_framework(dep))
                            .collect()
                    });
                (path, Some((facts, deps)))
            })
            .collect();
        for (path, entry) in read {
            let id = ids[&path] as usize;
            let Some((facts, deps)) = entry else { continue };
            let Some(deps) = deps.filter(|_| !facts.opaque) else {
                nodes[id].untrusted = true;
                continue;
            };
            nodes[id].launch = facts.launch;
            nodes[id].shared = facts.engine_shared;
            nodes[id].shardable = facts.shardable;
            for dep in deps {
                let next = u32::try_from(nodes.len()).unwrap_or(u32::MAX);
                let dep_id = *ids.entry(dep.clone()).or_insert_with(|| {
                    frontier.push(dep);
                    next
                });
                if dep_id == next {
                    nodes.push(Node::default());
                }
                nodes[id].deps.push(dep_id);
            }
        }
    }

    // A flag reaches every file that imports a flagged file: propagate it
    // backwards over the reversed edges, once per flag.
    let mut users: Vec<Vec<u32>> = vec![Vec::new(); nodes.len()];
    for (id, node) in nodes.iter().enumerate() {
        for &dep in &node.deps {
            users[dep as usize].push(u32::try_from(id).unwrap_or(u32::MAX));
        }
    }
    let reach = |seed: &dyn Fn(&Node) -> bool| -> Vec<bool> {
        let mut marked = vec![false; nodes.len()];
        let mut stack: Vec<u32> = Vec::new();
        for (id, node) in nodes.iter().enumerate() {
            if seed(node) {
                marked[id] = true;
                stack.push(u32::try_from(id).unwrap_or(u32::MAX));
            }
        }
        while let Some(id) = stack.pop() {
            for &user in &users[id as usize] {
                if !std::mem::replace(&mut marked[user as usize], true) {
                    stack.push(user);
                }
            }
        }
        marked
    };
    let to_stock = reach(&|node| node.launch || node.untrusted);
    let to_fresh = reach(&|node| node.shared);
    Ok(suites
        .iter()
        .map(|suite| {
            let id = ids[suite] as usize;
            // `shardable` is the suite file's own attestation — it never
            // propagates over the closure like `launch`/`shared` do.
            let shardable = nodes[id].shardable;
            let placement = if to_stock[id] {
                Placement::Stock
            } else if to_fresh[id] {
                Placement::Fresh
            } else {
                Placement::Shared
            };
            Hosting {
                placement,
                shardable,
            }
        })
        .collect())
}

/// A per-suite content digest over its whole import closure: the resolved
/// reachable files' bytes, plus the toolchain stamps a `flutter upgrade`
/// replaces in place. Deterministic across runs — it keys nothing on the
/// environment or on timing — so an unchanged tree digests identically and
/// a one-byte edit anywhere in the closure re-keys the chunk that suite
/// belongs to. This is the same resolution the run cache and [`place`]
/// perform (`package_config` → `lib_dirs`, framework files excluded — a
/// `flutter upgrade` is covered by the toolchain stamps instead); it is a
/// content digest, not a verdict, so it never refuses.
///
/// `None` only when the package configuration cannot be read at all — the
/// caller then keys the chunk on the suites' own file bytes as a floor.
#[must_use]
pub fn closure_digests(root: &Path, sdk: &Path, suites: &[PathBuf]) -> Option<Vec<String>> {
    use rayon::prelude::*;
    use rustc_hash::{FxHashMap, FxHashSet};

    let config_path = crate::cache::package_config_path(root);
    let text = std::fs::read_to_string(&config_path).ok()?;
    let config = dartq_pub::PackageConfig::parse(&text).ok()?;
    let dart_tool = config_path.parent()?;
    let packages = config.resolve(dart_tool).ok()?;
    let lib_dirs: FxHashMap<&str, &Path> = packages
        .iter()
        .map(|package| (package.name.as_str(), package.lib_dir.as_path()))
        .collect();
    let framework: Option<PathBuf> = config
        .additional_properties
        .get("flutterRoot")
        .and_then(serde_json::Value::as_str)
        .and_then(|uri| uri.strip_prefix("file://"))
        .map(PathBuf::from);
    let in_framework = |path: &Path| {
        framework
            .as_deref()
            .is_some_and(|sdk| path.starts_with(sdk))
    };

    // Breadth-first over the union closure, one frontier read in parallel.
    // Framework files are the toolchain every suite runs on and are covered
    // by the stamps below, not entered per suite.
    let mut ids: FxHashMap<PathBuf, u32> = FxHashMap::default();
    let mut file_of: Vec<PathBuf> = Vec::new();
    let mut deps_of: Vec<Vec<u32>> = Vec::new();
    let mut frontier: Vec<PathBuf> = Vec::new();
    for suite in suites {
        if !ids.contains_key(suite) {
            ids.insert(suite.clone(), file_of.len() as u32);
            file_of.push(suite.clone());
            deps_of.push(Vec::new());
            frontier.push(suite.clone());
        }
    }
    while !frontier.is_empty() {
        let read: Vec<(PathBuf, Option<Vec<PathBuf>>)> = frontier
            .par_drain(..)
            .map(|path| {
                let Ok(bytes) = std::fs::read(&path) else {
                    return (path, None);
                };
                let facts = crate::deps::facts(&bytes);
                let deps = facts
                    .uris
                    .iter()
                    .map(|uri| crate::cache::resolve_uri(&path, uri, &lib_dirs))
                    .collect::<Option<Vec<Option<PathBuf>>>>()
                    .map(|deps| {
                        deps.into_iter()
                            .flatten()
                            .filter(|dep| !in_framework(dep))
                            .collect()
                    });
                (path, deps)
            })
            .collect();
        for (path, deps) in read {
            let id = ids[&path] as usize;
            let Some(deps) = deps else { continue };
            for dep in deps {
                let dep_id = match ids.get(&dep) {
                    Some(&dep_id) => dep_id,
                    None => {
                        let dep_id = file_of.len() as u32;
                        ids.insert(dep.clone(), dep_id);
                        file_of.push(dep);
                        deps_of.push(Vec::new());
                        frontier.push(file_of[dep_id as usize].clone());
                        dep_id
                    }
                };
                deps_of[id].push(dep_id);
            }
        }
    }

    // Forward-reachable set per suite, then a digest over that set's file
    // bytes. Suites in one chunk share most of the closure, so per-file
    // digests are computed once across all suites.
    let suite_ids: Vec<u32> = suites.iter().map(|suite| ids[suite]).collect();
    let mut needed: FxHashSet<u32> = FxHashSet::default();
    {
        let mut stack = suite_ids.clone();
        while let Some(id) = stack.pop() {
            if needed.insert(id) {
                stack.extend(deps_of[id as usize].iter().copied());
            }
        }
    }
    let mut digest_of: FxHashMap<u32, [u8; 32]> = FxHashMap::default();
    for id in needed {
        let path = &file_of[id as usize];
        let mut leaf = blake3::Hasher::new();
        leaf.update(path.as_os_str().as_encoded_bytes());
        leaf.update(&std::fs::read(path).unwrap_or_default());
        digest_of.insert(id, *leaf.finalize().as_bytes());
    }

    // The toolchain stamps a `flutter upgrade` replaces in place (the same
    // files `Recipe::load` reads), so an upgrade re-keys every chunk even
    // with argv and package_config unchanged.
    let mut stamps = blake3::Hasher::new();
    for stamp in [
        "bin/cache/flutter.version.json",
        "bin/cache/engine.stamp",
        "bin/cache/dart-sdk/version",
    ] {
        stamps.update(&std::fs::read(sdk.join(stamp)).unwrap_or_default());
    }
    let stamps: [u8; 32] = *stamps.finalize().as_bytes();

    Some(
        suite_ids
            .iter()
            .map(|&seed| {
                let mut seen: FxHashSet<u32> = FxHashSet::default();
                let mut stack = vec![seed];
                while let Some(id) = stack.pop() {
                    if seen.insert(id) {
                        stack.extend(deps_of[id as usize].iter().copied());
                    }
                }
                // Feed the reachable files' digests in canonical order —
                // discovery order is nondeterministic (parallel frontier
                // reads), so the digest must not depend on it.
                let mut leaves: Vec<[u8; 32]> = seen
                    .iter()
                    .map(|id| digest_of.get(id).copied().unwrap_or([0u8; 32]))
                    .collect();
                leaves.sort_unstable();
                let mut hasher = blake3::Hasher::new();
                hasher.update(&stamps);
                for leaf in leaves {
                    hasher.update(&leaf);
                }
                hasher.finalize().to_hex().to_string()
            })
            .collect(),
    )
}

impl Recipe {
    /// The recipe for `args`, from the checkout's cache when the toolchain,
    /// the arguments, the package configuration and the environment are
    /// what they were when it was probed; else probed afresh and cached.
    ///
    /// # Errors
    ///
    /// I/O failures reading the toolchain stamps or running the probe.
    pub fn load(flutter: &Path, root: &Path, args: &[String]) -> io::Result<Option<Self>> {
        let mut hasher = blake3::Hasher::new();
        let sdk = flutter
            .canonicalize()?
            .parent()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .unwrap_or_default();
        for stamp in [
            "bin/cache/flutter.version.json",
            "bin/cache/engine.stamp",
            "bin/cache/dart-sdk/version",
        ] {
            hasher.update(stamp.as_bytes());
            hasher.update(&std::fs::read(sdk.join(stamp)).unwrap_or_default());
        }
        hasher.update(root.to_string_lossy().as_bytes());
        hasher.update(&std::fs::read(crate::cache::package_config_path(root)).unwrap_or_default());
        hasher.update(&std::fs::read(root.join("pubspec.yaml")).unwrap_or_default());
        for arg in args {
            hasher.update(&(arg.len() as u64).to_le_bytes());
            hasher.update(arg.as_bytes());
        }
        let mut env: Vec<(std::ffi::OsString, std::ffi::OsString)> = std::env::vars_os().collect();
        env.sort_unstable();
        for (name, value) in env {
            hasher.update(name.as_encoded_bytes());
            hasher.update(b"=");
            hasher.update(value.as_encoded_bytes());
            hasher.update(b"\0");
        }
        // The probe is also the tool's asset preparation (the bundle under
        // `build/unit_test_assets`, native assets): the recipe stands only
        // while every asset input and the prepared bundle are unchanged,
        // and never for packages whose build hooks the tool reruns.
        if has_build_hooks(root) {
            return Self::probe(flutter, root, args);
        }
        for asset in crate::cache::flutter_assets(root) {
            hasher.update(asset.to_string_lossy().as_bytes());
            hasher.update(&std::fs::read(&asset).unwrap_or_default());
        }
        // The prepared bundle is the probe's other output: its *bytes* key
        // it, not len+mtime — the tool touches it on every one of its own
        // runs without changing contents, and mtime would re-probe forever.
        let bundle = root.join("build").join("unit_test_assets");
        for name in [
            "AssetManifest.bin",
            "FontManifest.json",
            "NativeAssetsManifest.json",
        ] {
            hasher.update(name.as_bytes());
            hasher.update(&std::fs::read(bundle.join(name)).unwrap_or_default());
        }
        let key = hasher.finalize().to_hex().to_string();
        let path = work_dir(root).join("recipe.json");
        #[derive(Serialize, Deserialize)]
        struct Stored {
            key: String,
            recipe: Recipe,
        }
        if let Some(stored) = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Stored>(&bytes).ok())
        {
            if stored.key == key {
                return Ok(Some(stored.recipe));
            }
        }
        let Some(recipe) = Self::probe(flutter, root, args)? else {
            return Ok(None);
        };
        let bytes = serde_json::to_vec(&Stored {
            key,
            recipe: recipe.clone(),
        })
        .map_err(io::Error::other)?;
        crate::stamp::write_atomic(&path, &bytes)?;
        Ok(Some(recipe))
    }
}

/// Any resolved package ships a `hook/build.dart`: the tool runs its
/// native-asset build before every run.
fn has_build_hooks(root: &Path) -> bool {
    let config_path = crate::cache::package_config_path(root);
    let Ok(text) = std::fs::read_to_string(&config_path) else {
        return true;
    };
    let Ok(config) = dartq_pub::PackageConfig::parse(&text) else {
        return true;
    };
    let Some(dart_tool) = config_path.parent() else {
        return true;
    };
    let Ok(packages) = config.resolve(dart_tool) else {
        return true;
    };
    packages
        .iter()
        .any(|package| package.root.join("hook").join("build.dart").is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TRACE: &str = r#"[   +3 ms] running test package with arguments: [--no-color, -r, json, --chain-stack-traces, --, file:///p/probe_test.dart?]
[  +20 ms] /sdk/bin/cache/dart-sdk/bin/dartaotruntime /sdk/bin/cache/dart-sdk/bin/snapshots/frontend_server_aot.dart.snapshot --sdk-root /sdk/bin/cache/artifacts/engine/common/flutter_patched_sdk/ --incremental --no-print-incremental-dependencies --target=flutter -DFLUTTER_VERSION=3.47.1 --output-dill /t/output.dill --packages /p/.dart_tool/package_config.json -Ddart.vm.profile=false --enable-asserts --track-widget-creation --initialize-from-dill /p/build/x.dill --verbosity=error --enable-experiment=alternative-invalidation-strategy
[        ] test 0: Starting flutter_tester process with command=[/sdk/bin/cache/artifacts/engine/darwin-x64/flutter_tester, --disable-vm-service, --enable-checked-mode, --non-interactive, --packages=/p/.dart_tool/package_config.json, --flutter-assets-dir=/p/build/unit_test_assets, /t/listener.dart.dill], environment={FLUTTER_TEST: true, FONTCONFIG_FILE: /t/fonts.conf, SERVER_PORT: 1, APP_NAME: p}
{"testID":3,"messageType":"print","message":"dartq.environment {\"FLUTTER_TEST\":\"true\",\"FONTCONFIG_FILE\":\"/t/fonts.conf\",\"SERVER_PORT\":\"1\",\"APP_NAME\":\"p\",\"HOME\":\"/h\"}","type":"print","time":10}
"#;

    #[test]
    fn recipe_follows_the_tools_trace() {
        let recipe = Recipe::parse(TRACE).expect("recipe");
        assert_eq!(
            recipe.compiler[0],
            "/sdk/bin/cache/dart-sdk/bin/dartaotruntime"
        );
        // Per-run arguments are dartq's; everything else is the tool's.
        for dropped in [
            "--incremental",
            "--output-dill",
            "/t/output.dill",
            "--packages",
            "--initialize-from-dill",
            "/p/build/x.dill",
        ] {
            assert!(
                !recipe.compiler.iter().any(|arg| arg == dropped),
                "{dropped}"
            );
        }
        // The seed path survives as the stock cohort's pre-seed target.
        assert_eq!(recipe.test_cache, PathBuf::from("/p/build/x.dill"));
        for kept in [
            "--target=flutter",
            "-DFLUTTER_VERSION=3.47.1",
            "--track-widget-creation",
            "--enable-experiment=alternative-invalidation-strategy",
        ] {
            assert!(recipe.compiler.iter().any(|arg| arg == kept), "{kept}");
        }
        assert_eq!(
            recipe.tester.last().map(String::as_str),
            Some("--flutter-assets-dir=/p/build/unit_test_assets")
        );
        // The tester environment is what the probe observed, minus the
        // runner port and the fonts file dartq writes itself.
        assert_eq!(
            recipe.environment,
            [("APP_NAME", "p"), ("FLUTTER_TEST", "true"), ("HOME", "/h")]
                .map(|(k, v)| (k.to_owned(), v.to_owned()))
        );
        assert!(recipe.fonts.contains("<dir>/sdk/bin/cache/artifacts</dir>"));
        // A tester argument the trace cannot split back is refused.
        assert!(Recipe::parse(&TRACE.replace("--non-interactive", "a, b")).is_none());
        assert!(Recipe::parse(&TRACE.replace("dartq.environment", "other")).is_none());
    }

    #[test]
    fn chunk_entries_wire_each_suite_like_the_stock_bootstrap() {
        let plain = Hosted {
            path: PathBuf::from("/p/test/a b_test.dart"),
            config: None,
            fresh: false,
            digest: None,
            shard_total: 1,
        };
        let wrapped = Hosted {
            path: PathBuf::from("/p/test/w/x_test.dart"),
            config: Some(PathBuf::from("/p/test/flutter_test_config.dart")),
            fresh: false,
            digest: None,
            shard_total: 1,
        };
        let src = chunk_entry(3, &[(0, &plain), (2, &wrapped)]);
        assert!(src.contains("import 'file:///p/test/a%20b_test.dart' as s0;"));
        assert!(src.contains("import 'file:///p/test/flutter_test_config.dart' as c0;"));
        assert!(src.contains("const int _chunk = 3;"));
        assert!(src.contains("void _s0() => _connect(0, 'file:///p/test/a%20b_test.dart', () => () async { await Future(s0.main); });"));
        assert!(src.contains("void _s2() => _connect(2, 'file:///p/test/w/x_test.dart', () => () => c0.testExecutable(() async { await Future(s2.main); }));"));
        // Imports precede every declaration.
        let first_decl = src.find("@Native").expect("declarations");
        assert!(src.find("as s2;").expect("import") < first_decl);
    }

    #[test]
    fn placement_follows_the_import_closure() {
        let tmp = tempfile::tempdir().expect("tmp");
        let root = tmp.path().canonicalize().expect("canonical");
        let write = |rel: &str, text: &str| {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
            std::fs::write(&path, text).expect("write");
            path
        };
        write(
            ".dart_tool/package_config.json",
            r#"{"configVersion":2,"packages":[{"name":"p","rootUri":"../","packageUri":"lib/"}]}"#,
        );
        write("lib/fonts.dart", "void load() { FontLoader('x'); }");
        write("lib/via.dart", "import 'fonts.dart';\nvoid f() {}");
        write(
            "lib/script.dart",
            "import 'dart:io';\nvar s = Platform.script;",
        );
        let plain = write(
            "test/a_test.dart",
            "import 'package:p/missing.dart';\nvoid main() {}",
        );
        let fresh = write(
            "test/b_test.dart",
            "import 'package:p/via.dart';\nvoid main() {}",
        );
        let stock = write(
            "test/c_test.dart",
            "import '../lib/script.dart';\nvoid main() {}",
        );
        let opaque = write("test/d_test.dart", "import 'x$y.dart';\nvoid main() {}");
        let placements: Vec<Placement> = place(&root, &[plain, fresh, stock, opaque])
            .expect("placed")
            .iter()
            .map(|hosting| hosting.placement)
            .collect();
        assert_eq!(
            placements,
            [
                Placement::Shared,
                Placement::Fresh,
                Placement::Stock,
                Placement::Stock
            ]
        );
    }

    /// `shardable` is read from the suite's own file only: a marker in a
    /// helper the suite imports means nothing, and the suite file's own
    /// marker is not propagated to anyone importing it.
    #[test]
    fn shardable_attests_the_suite_file_only() {
        let tmp = tempfile::tempdir().expect("tmp");
        let root = tmp.path().canonicalize().expect("canonical");
        let write = |rel: &str, text: &str| {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
            std::fs::write(&path, text).expect("write");
            path
        };
        write(
            ".dart_tool/package_config.json",
            r#"{"configVersion":2,"packages":[{"name":"p","rootUri":"../","packageUri":"lib/"}]}"#,
        );
        write("lib/marked.dart", "// dartq: shardable\nvoid f() {}");
        let marked = write(
            "test/marked_test.dart",
            "// dartq: shardable both halves are independent\nvoid main() {}",
        );
        let via_helper = write(
            "test/helper_test.dart",
            "import 'package:p/marked.dart';\nvoid main() {}",
        );
        let plain = write("test/plain_test.dart", "void main() {}");
        let hostings = place(&root, &[marked, via_helper, plain]).expect("placed");
        assert_eq!(
            hostings
                .iter()
                .map(|hosting| hosting.shardable)
                .collect::<Vec<_>>(),
            [true, false, false]
        );
    }

    /// A trace dartq cannot split back unambiguously, or one missing a part
    /// of the recipe, is refused rather than guessed.
    #[test]
    fn recipe_parse_refuses_ambiguous_or_incomplete_traces() {
        let drop_line = |needle: &str| {
            TRACE
                .lines()
                .filter(|line| !line.contains(needle))
                .collect::<Vec<_>>()
                .join("\n")
        };
        // The compiler line, the tester line and the environment event are
        // all required.
        assert!(Recipe::parse(&drop_line("frontend_server")).is_none());
        assert!(Recipe::parse(&drop_line("Starting flutter_tester")).is_none());
        assert!(Recipe::parse(&drop_line("dartq.environment")).is_none());
        // A double space cannot be split back into arguments.
        assert!(Recipe::parse(&TRACE.replace("--sdk-root ", "--sdk-root  ")).is_none());
        // A tester argument that is not a flag means the split guessed.
        assert!(Recipe::parse(&TRACE.replace("--non-interactive", "positional")).is_none());
        // An environment that is not a map of strings is refused.
        let swapped = TRACE
            .lines()
            .map(|line| {
                if line.contains("dartq.environment") {
                    r#"{"testID":3,"messageType":"print","message":"dartq.environment [1]","type":"print","time":10}"#
                } else {
                    line
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(Recipe::parse(&swapped).is_none());
        // The fonts configuration is derived from the compiler's SDK cache;
        // a compiler outside one yields no recipe.
        assert!(
            Recipe::parse(&TRACE.replace(
                "/sdk/bin/cache/dart-sdk/bin/dartaotruntime",
                "/opt/dart/dartaotruntime"
            ))
            .is_none()
        );
    }

    /// Suites wrapped by the same `flutter_test_config.dart` share its
    /// import: each config imports once however many suites use it.
    #[test]
    fn chunk_entry_imports_each_config_once() {
        let one = PathBuf::from("/p/test/one/flutter_test_config.dart");
        let top = PathBuf::from("/p/test/flutter_test_config.dart");
        let wrapped = |path: &str, config: &Path| Hosted {
            path: PathBuf::from(path),
            config: Some(config.to_path_buf()),
            fresh: false,
            digest: None,
            shard_total: 1,
        };
        let a = wrapped("/p/test/one/a_test.dart", &one);
        let b = wrapped("/p/test/one/b_test.dart", &one);
        let c = wrapped("/p/test/c_test.dart", &top);
        let src = chunk_entry(1, &[(0, &a), (2, &b), (3, &c)]);
        assert_eq!(src.matches("as c0;").count(), 1);
        assert_eq!(src.matches("as c1;").count(), 1);
        assert!(src.contains("const int _chunk = 1;"));
        for (suite, config) in [("s0", "c0"), ("s2", "c0"), ("s3", "c1")] {
            assert!(
                src.contains(&format!(
                    "{config}.testExecutable(() async {{ await Future({suite}.main); }})"
                )),
                "{suite}"
            );
        }
    }

    /// A suite importing a package the resolution does not know runs stock
    /// (its closure cannot be read); the Flutter SDK's own packages are the
    /// framework every suite runs on and are never entered.
    #[test]
    fn placement_stocks_unresolved_and_skips_the_framework() {
        let tmp = tempfile::tempdir().expect("tmp");
        let root = tmp.path().canonicalize().expect("canonical");
        let write = |base: &Path, rel: &str, text: &str| {
            let path = base.join(rel);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
            std::fs::write(&path, text).expect("write");
            path
        };
        // Outside the package, spelled as canonically as the resolved
        // imports will be (the framework prefix check compares paths).
        let sdk = root.parent().expect("tmp parent").join("sdk");
        write(
            &sdk,
            "packages/fw/lib/launch.dart",
            "import 'dart:io';\nvar s = Platform.script;\n",
        );
        let config = format!(
            concat!(
                r#"{{"configVersion":2,"packages":["#,
                r#"{{"name":"p","rootUri":"../","packageUri":"lib/"}},"#,
                r#"{{"name":"fw","rootUri":"../sdk/packages/fw/","packageUri":"lib/"}}],"#,
                r#""flutterRoot":"file://{sdk}"}}"#
            ),
            sdk = sdk.display()
        );
        std::fs::create_dir_all(root.join(".dart_tool")).expect("dir");
        std::fs::write(root.join(".dart_tool/package_config.json"), config).expect("write");
        let own = write(
            &root,
            "test/own_test.dart",
            "void main() { FontLoader('x'); }",
        );
        let unknown = write(
            &root,
            "test/unknown_test.dart",
            "import 'package:nosuch/x.dart';\nvoid main() {}",
        );
        let framework = write(
            &root,
            "test/fw_test.dart",
            "import 'package:fw/launch.dart';\nvoid main() {}",
        );
        let placements: Vec<Placement> = place(&root, &[own, unknown, framework])
            .expect("placed")
            .iter()
            .map(|hosting| hosting.placement)
            .collect();
        assert_eq!(
            placements,
            [Placement::Fresh, Placement::Stock, Placement::Shared]
        );
    }

    /// The chunk key is a content identity: identical inputs produce the
    /// same key (cross-run adoption), while a changed resolution, a
    /// different suite set, or a different compiler command re-keys to a
    /// fresh compile. Anything not keyed would serve a stale kernel.
    #[test]
    fn chunk_key_rekeys_on_every_input() {
        let tmp = tempfile::tempdir().expect("tmp");
        let packages = tmp.path().join("package_config.json");
        std::fs::write(&packages, br#"{"configVersion":2,"packages":[]}"#).expect("write");
        let recipe = Recipe {
            compiler: vec!["fe".to_owned(), "--target=flutter".to_owned()],
            test_cache: PathBuf::from("/t/cache.dill"),
            tester: vec!["tester".to_owned()],
            environment: Vec::new(),
            fonts: String::new(),
        };
        // Each suite carries a run-cache key (the closure+config+toolchain
        // digest); the chunk key folds it verbatim.
        let suites = [
            Hosted {
                path: PathBuf::from("/p/test/a_test.dart"),
                config: None,
                fresh: false,
                digest: Some("cachekey-a".to_owned()),
                shard_total: 1,
            },
            Hosted {
                path: PathBuf::from("/p/test/b_test.dart"),
                config: None,
                fresh: false,
                digest: Some("cachekey-b".to_owned()),
                shard_total: 1,
            },
        ];
        let plan = Plan {
            root: tmp.path(),
            vm: Path::new("/vm"),
            tool_packages: Path::new("/tool"),
            packages: &packages,
            recipe: &recipe,
            suites: &suites,
            runner_args: &[],
            concurrency: 1,
        };
        let members = vec![0usize, 1usize];
        let locals = vec![0usize, 1usize];
        let key = chunk_key(&plan, 0, &locals, |local| members[local]);
        assert_eq!(
            key,
            chunk_key(&plan, 0, &locals, |local| members[local]),
            "same inputs same key"
        );
        // The resolution bytes are an input.
        std::fs::write(
            &packages,
            br#"{"configVersion":2,"packages":[{"name":"x","rootUri":"../","packageUri":"lib/"}]}"#,
        )
        .expect("write");
        assert_ne!(
            key,
            chunk_key(&plan, 0, &locals, |local| members[local]),
            "resolution change re-keys"
        );
        std::fs::write(&packages, br#"{"configVersion":2,"packages":[]}"#).expect("write");
        // A different member set re-keys (and a retried subset differs from
        // the full set it was reduced from).
        let subset = vec![0usize];
        assert_ne!(
            key,
            chunk_key(&plan, 0, &subset, |local| members[local]),
            "member set re-keys"
        );
        // A different chunk index re-keys (the entry's `_chunk` differs).
        assert_ne!(
            key,
            chunk_key(&plan, 1, &locals, |local| members[local]),
            "chunk index re-keys"
        );
        // A different compiler command re-keys.
        let other_recipe = Recipe {
            compiler: vec!["fe".to_owned(), "--target=flutter2".to_owned()],
            ..recipe.clone()
        };
        let other_plan = Plan {
            recipe: &other_recipe,
            ..plan
        };
        assert_ne!(
            key,
            chunk_key(&other_plan, 0, &locals, |local| members[local]),
            "compiler command re-keys"
        );
        // A changed run-cache key re-keys (that key is what carries a
        // source/lib/dependency/config byte change into the chunk).
        let moved = [
            Hosted {
                digest: Some("cachekey-a2".to_owned()),
                ..suites[0].clone()
            },
            suites[1].clone(),
        ];
        let moved_plan = Plan {
            suites: &moved,
            ..plan
        };
        assert_ne!(
            key,
            chunk_key(&moved_plan, 0, &locals, |local| members[local]),
            "suite cache-key change re-keys"
        );
    }

    /// A byte flip inside a suite's import closure — not an entry input —
    /// re-keys the chunk because the suite's run-cache key (which folds the
    /// closure's content digests) changes. With no cache key to lean on, the
    /// suite's own file bytes are the floor and re-key on any edit too.
    #[test]
    fn chunk_key_rekeys_on_a_closure_or_source_byte_change() {
        let tmp = tempfile::tempdir().expect("tmp");
        let packages = tmp.path().join("package_config.json");
        std::fs::write(&packages, br#"{"configVersion":2,"packages":[]}"#).expect("write");
        let suite_file = tmp.path().join("a_test.dart");
        std::fs::write(&suite_file, b"void main() {}").expect("write");
        let recipe = Recipe {
            compiler: vec!["fe".to_owned()],
            test_cache: PathBuf::from("/t/cache.dill"),
            tester: vec![],
            environment: Vec::new(),
            fonts: String::new(),
        };
        // No run-cache key: the file bytes are the chunk key's floor.
        let no_key = [Hosted {
            path: suite_file.clone(),
            config: None,
            fresh: false,
            digest: None,
            shard_total: 1,
        }];
        let plan = Plan {
            root: tmp.path(),
            vm: Path::new("/vm"),
            tool_packages: Path::new("/tool"),
            packages: &packages,
            recipe: &recipe,
            suites: &no_key,
            runner_args: &[],
            concurrency: 1,
        };
        let members = vec![0usize];
        let locals = vec![0usize];
        let before = chunk_key(&plan, 0, &locals, |local| members[local]);
        std::fs::write(&suite_file, b"void main() {}\n").expect("write");
        assert_ne!(
            before,
            chunk_key(&plan, 0, &locals, |local| members[local]),
            "a source byte re-keys"
        );
        // With a run-cache key the file bytes are no longer hashed — the key
        // is. A changed cache key re-keys even when file bytes are identical.
        let with_key = [Hosted {
            digest: Some("closure-digest-v1".to_owned()),
            ..no_key[0].clone()
        }];
        let plan = Plan {
            suites: &with_key,
            ..plan
        };
        let k1 = chunk_key(&plan, 0, &locals, |local| members[local]);
        let with_key2 = [Hosted {
            digest: Some("closure-digest-v2".to_owned()),
            ..no_key[0].clone()
        }];
        let plan = Plan {
            suites: &with_key2,
            ..plan
        };
        assert_ne!(
            k1,
            chunk_key(&plan, 0, &locals, |local| members[local]),
            "a closure digest change re-keys"
        );
    }

    /// A `flutter_test_config.dart` is read as bytes when the suite has no
    /// run-cache key; a byte flip in it re-keys the chunk. (With a cache key
    /// the config's closure is already inside it — the previous test.)
    #[test]
    fn chunk_key_rekeys_on_a_config_byte_change() {
        let tmp = tempfile::tempdir().expect("tmp");
        let packages = tmp.path().join("package_config.json");
        std::fs::write(&packages, br#"{"configVersion":2,"packages":[]}"#).expect("write");
        let suite_file = tmp.path().join("w_test.dart");
        let config_file = tmp.path().join("flutter_test_config.dart");
        std::fs::write(&suite_file, b"void main() {}").expect("write");
        std::fs::write(&config_file, b"// config v1").expect("write");
        let recipe = Recipe {
            compiler: vec!["fe".to_owned()],
            test_cache: PathBuf::from("/t/cache.dill"),
            tester: vec![],
            environment: vec![],
            fonts: String::new(),
        };
        let suites = [Hosted {
            path: suite_file,
            config: Some(config_file.clone()),
            fresh: false,
            digest: None,
            shard_total: 1,
        }];
        let plan = Plan {
            root: tmp.path(),
            vm: Path::new("/vm"),
            tool_packages: Path::new("/tool"),
            packages: &packages,
            recipe: &recipe,
            suites: &suites,
            runner_args: &[],
            concurrency: 1,
        };
        let members = vec![0usize];
        let locals = vec![0usize];
        let before = chunk_key(&plan, 0, &locals, |local| members[local]);
        std::fs::write(&config_file, b"// config v2").expect("write");
        assert_ne!(
            before,
            chunk_key(&plan, 0, &locals, |local| members[local]),
            "a config byte re-keys"
        );
    }

    /// `closure_digests` digests every file a suite's imports reach: a byte
    /// flip in a `lib/` source the suite transitively imports — a file that
    /// is *not* one of the chunk entry's inputs — changes that suite's
    /// digest, which re-keys its chunk. An unchanged tree digests
    /// identically across calls (deterministic, no env or timing inputs).
    #[test]
    fn closure_digests_rekey_on_a_transitively_imported_byte() {
        let tmp = tempfile::tempdir().expect("tmp");
        let root = tmp.path().canonicalize().expect("canon");
        let write = |rel: &str, text: &str| {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
            std::fs::write(&path, text).expect("write");
            path
        };
        write(
            ".dart_tool/package_config.json",
            r#"{"configVersion":2,"packages":[{"name":"p","rootUri":"../","packageUri":"lib/"}]}"#,
        );
        let dep = write("lib/helper.dart", "int answer() => 41;");
        let suite = write(
            "test/a_test.dart",
            "import 'package:p/helper.dart';\nvoid main() {}",
        );
        let sdk = root.join("sdk");
        std::fs::create_dir_all(sdk.join("bin/cache/dart-sdk")).expect("sdk dirs");
        std::fs::write(sdk.join("bin/cache/dart-sdk/version"), b"3.13.1").expect("version");

        let first =
            closure_digests(&root, &sdk, std::slice::from_ref(&suite)).expect("digests")[0].clone();
        // Same tree, same digest — deterministic across calls.
        let again =
            closure_digests(&root, &sdk, std::slice::from_ref(&suite)).expect("digests")[0].clone();
        assert_eq!(first, again, "an unchanged tree digests identically");
        // One byte in a transitively-imported lib file re-keys the suite.
        std::fs::write(&dep, "int answer() => 42;").expect("write");
        let changed =
            closure_digests(&root, &sdk, std::slice::from_ref(&suite)).expect("digests")[0].clone();
        assert_ne!(first, changed, "a lib byte flip re-keys the suite");
        // And the toolchain stamp is an input: a version bump re-keys even
        // with argv and package_config untouched.
        std::fs::write(sdk.join("bin/cache/dart-sdk/version"), b"3.13.2").expect("version");
        let upgraded =
            closure_digests(&root, &sdk, std::slice::from_ref(&suite)).expect("digests")[0].clone();
        assert_ne!(changed, upgraded, "a toolchain stamp bump re-keys");
    }

    /// `seed_test_cache` copies the largest chunk kernel under the work dir
    /// into the tool's shared seed path — but never displaces a seed that is
    /// already at least as large (the tool's own largest-kernel rule).
    #[test]
    fn seed_test_cache_writes_the_largest_chunk_only_when_smaller() {
        let tmp = tempfile::tempdir().expect("tmp");
        let root = tmp.path().canonicalize().expect("canon");
        let dir = work_dir(&root);
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(dir.join("chunk0.dill"), b"small").expect("kernel");
        std::fs::write(dir.join("chunk1.dill"), b"the largest kernel").expect("kernel");
        // Sidecars and seeds are not candidates.
        std::fs::write(dir.join("chunk2.dill.key"), b"key").expect("key");
        std::fs::write(dir.join("chunk3.seed.dill"), b"stale seed").expect("seed");
        std::fs::write(dir.join("compile.dill"), b"compiler output").expect("out");
        let recipe = Recipe {
            compiler: vec!["fe".to_owned()],
            test_cache: root.join("build/test_cache/build/x.cache.dill.track.dill"),
            tester: vec![],
            environment: vec![],
            fonts: String::new(),
        };
        seed_test_cache(&recipe, &root);
        assert_eq!(
            std::fs::read(&recipe.test_cache).expect("seeded"),
            b"the largest kernel",
            "the largest chunk kernel seeds the tool's cache"
        );
        // A same-or-larger existing seed is kept.
        std::fs::write(&recipe.test_cache, b"an even larger existing seed file").expect("write");
        seed_test_cache(&recipe, &root);
        assert_eq!(
            std::fs::read(&recipe.test_cache).expect("kept"),
            b"an even larger existing seed file"
        );
        // No seed path, no work — and no stray file beside the root.
        let quiet = Recipe {
            test_cache: PathBuf::new(),
            ..recipe.clone()
        };
        seed_test_cache(&quiet, &root);
        assert!(!root.join("seed-1").exists());
    }
}
