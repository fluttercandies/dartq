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
    match line.strip_prefix('[').and_then(|rest| rest.split_once("] ")) {
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
                if let Some(text) = event.str_at(&["message"]).and_then(|message| message.strip_prefix(ENVIRONMENT)) {
                    environment = Some(serde_json::from_str::<std::collections::BTreeMap<String, String>>(text).ok()?);
                }
                continue;
            }
            let body = trace_body(line);
            if compiler.is_none() && body.contains("frontend_server") && body.contains(" --sdk-root ") {
                compiler = Some(split_trace(body)?);
            } else if let Some(rest) = body.strip_prefix("test 0: Starting flutter_tester process with command=") {
                let (command, _) = rest.split_once("], environment=")?;
                let command = split_list(&format!("{command}]"))?;
                // Every argument after the binary is a flag, but the last
                // (the kernel): anything else means the split was ambiguous.
                if command.len() < 2 || command[1..command.len() - 1].iter().any(|arg| !arg.starts_with("--")) {
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
        // Per-run arguments dartq supplies itself.
        let mut kept = Vec::with_capacity(compiler.len());
        let mut parts = compiler.into_iter();
        while let Some(part) = parts.next() {
            match part.as_str() {
                "--output-dill" | "--initialize-from-dill" | "--packages" => {
                    parts.next()?;
                }
                "--incremental" => {}
                _ => kept.push(part),
            }
        }
        let environment: Vec<(String, String)> = environment
            .into_iter()
            .filter(|(key, _)| key != "SERVER_PORT" && key != "FONTCONFIG_FILE")
            .collect();
        // The tool deletes its fonts configuration on exit; the contents are
        // fixed (`FontConfigManager`): the SDK's artifact cache directory.
        let artifacts = Path::new(kept.first()?).ancestors().find(|dir| dir.ends_with("bin/cache"))?.join("artifacts");
        let fonts = format!(
            "<fontconfig>\n  <dir>{}</dir>\n  <cachedir>/var/cache/fontconfig</cachedir>\n</fontconfig>\n",
            artifacts.display()
        );
        Some(Self { compiler: kept, tester: command, environment, fonts })
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
        let _ = writeln!(header, "import {} as s{i};", crate::group::dart_string(&file_uri(&suite.path)));
        if let Some(config) = &suite.config {
            if !configs.contains(&config.as_path()) {
                let _ = writeln!(header, "import {} as c{};", crate::group::dart_string(&file_uri(config)), configs.len());
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
                let c = configs.iter().position(|seen| *seen == config.as_path()).expect("imported above");
                format!("() => () => c{c}.testExecutable(() async {{ await Future(s{i}.main); }})")
            }
            None => format!("() => () async {{ await Future(s{i}.main); }}"),
        };
        let _ = writeln!(src, "\n@pragma('vm:entry-point')\nvoid _s{i}() => _connect({i}, {url}, {main});");
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
    fn start(recipe: &Recipe, packages: &Path, output: &Path, root: &Path) -> io::Result<Self> {
        let (program, args) = recipe.compiler.split_first().ok_or_else(|| io::Error::other("empty compiler command"))?;
        let mut child = Command::new(program)
            .args(args)
            .arg("--incremental")
            .arg("--packages")
            .arg(packages)
            .arg("--output-dill")
            .arg(output)
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let stdin = child.stdin.take().expect("piped");
        let stdout = BufReader::new(child.stdout.take().expect("piped"));
        Ok(Self { child, stdin, stdout, output: output.to_path_buf(), first: true })
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
                        let errors: usize = rest.rsplit(' ').next().and_then(|n| n.parse().ok()).unwrap_or(1);
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
    let dir = crate::host::cache_dir().map_or_else(|| scratch.to_path_buf(), |dir| dir.join("test").join("flutter"));
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
    let chunks: Vec<Vec<usize>> = (0..plan.suites.len()).collect::<Vec<_>>().chunks(CHUNK).map(<[usize]>::to_vec).collect();
    let kernels: Vec<PathBuf> = (0..chunks.len()).map(|index| dir.join(format!("chunk{index}.dill"))).collect();
    for kernel in &kernels {
        for stale in [kernel.clone(), kernel.with_extension("dill.failed"), kernel.with_extension("dill.json")] {
            let _ = std::fs::remove_file(stale);
        }
    }
    let fonts = dir.join("fonts.conf");
    crate::stamp::write_atomic(&fonts, plan.recipe.fonts.as_bytes())?;
    let mut environment: serde_json::Map<String, serde_json::Value> =
        plan.recipe.environment.iter().map(|(key, value)| (key.clone(), value.clone().into())).collect();
    environment.insert("FONTCONFIG_FILE".to_owned(), fonts.display().to_string().into());
    let mut rows = Vec::with_capacity(plan.suites.len());
    for (chunk, members) in chunks.iter().enumerate() {
        for (local, &index) in members.iter().enumerate() {
            let suite = &plan.suites[index];
            rows.push(serde_json::json!([suite.path.display().to_string(), chunk, local, suite.fresh]));
        }
    }
    let mut args: Vec<String> = plan.runner_args.to_vec();
    args.extend(["--reporter".to_owned(), "json".to_owned(), "--".to_owned()]);
    args.extend(plan.suites.iter().map(|suite| suite.path.display().to_string()));
    let config = serde_json::json!({
        "tester": plan.recipe.tester,
        "environment": environment,
        "chunks": kernels.iter().map(|kernel| kernel.display().to_string()).collect::<Vec<_>>(),
        "concurrency": plan.concurrency.max(1),
        "suites": rows,
        "args": args,
    });
    let config_path = dir.join("run.json");
    crate::stamp::write_atomic(&config_path, config.to_string().as_bytes())?;

    let excluded = std::sync::Mutex::new(Vec::new());
    let stop = std::sync::atomic::AtomicBool::new(false);
    let result = std::thread::scope(|scope| {
        let compiling = scope.spawn(|| compile_chunks(plan, &dir, &chunks, &kernels, &excluded, &stop));
        let mut cmd = Command::new(plan.vm);
        cmd.arg("--disable-dart-dev")
            .arg(&runner)
            .arg(&config_path)
            .current_dir(plan.root)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let ran = crate::exec::run_command_for(cmd, cfg);
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let compiled = compiling.join().unwrap_or_else(|_| Err(io::Error::other("the chunk compiler panicked")));
        (ran, compiled)
    });
    let (ran, compiled) = result;
    compiled?;
    Ok(Outcome { execution: ran?, excluded: excluded.into_inner().unwrap_or_default() })
}

/// Compile every chunk in order through one resident compiler, writing
/// each kernel (then its `.json` list of hosted suites) once whole, or a
/// `.failed` marker. A suite whose own file fails is dropped and its chunk
/// recompiled once without it; its row in the runner stays, and loading
/// it fails, which the caller reruns stock.
fn compile_chunks(
    plan: &Plan<'_>,
    dir: &Path,
    chunks: &[Vec<usize>],
    kernels: &[PathBuf],
    excluded: &std::sync::Mutex<Vec<PathBuf>>,
    stop: &std::sync::atomic::AtomicBool,
) -> io::Result<()> {
    let output = dir.join("compile.dill");
    let mut compiler: Option<Compiler> = None;
    for (index, members) in chunks.iter().enumerate() {
        if stop.load(std::sync::atomic::Ordering::Relaxed) {
            break;
        }
        let mut hosted: Vec<usize> = (0..members.len()).collect();
        let mut retried = false;
        loop {
            let suites: Vec<(usize, &Hosted)> = hosted.iter().map(|&local| (local, &plan.suites[members[local]])).collect();
            let entry = dir.join(format!("chunk{index}.dart"));
            crate::stamp::write_atomic(&entry, chunk_entry(index, &suites).as_bytes())?;
            if compiler.is_none() {
                compiler = Some(Compiler::start(plan.recipe, plan.packages, &output, plan.root)?);
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
                    crate::stamp::write_atomic(&kernels[index].with_extension("dill.json"), serde_json::to_string(&hosted)?.as_bytes())?;
                    std::fs::rename(&output, &kernels[index])?;
                    break;
                }
                Compiled::Errors(text) if !retried => {
                    let paths: Vec<PathBuf> = hosted.iter().map(|&local| plan.suites[members[local]].path.clone()).collect();
                    let broken = crate::group::broken_suites(plan.root, &paths, &text);
                    match broken.filter(|broken| broken.len() < paths.len()) {
                        Some(broken) => {
                            hosted.retain(|&local| !broken.contains(&plan.suites[members[local]].path));
                            excluded.lock().map_err(|_| io::Error::other("poisoned"))?.extend(broken);
                            retried = true;
                        }
                        None => {
                            fail_chunk(plan, members, &hosted, &kernels[index], excluded)?;
                            break;
                        }
                    }
                }
                Compiled::Errors(_) => {
                    fail_chunk(plan, members, &hosted, &kernels[index], excluded)?;
                    break;
                }
            }
        }
    }
    Ok(())
}

fn fail_chunk(
    plan: &Plan<'_>,
    members: &[usize],
    hosted: &[usize],
    kernel: &Path,
    excluded: &std::sync::Mutex<Vec<PathBuf>>,
) -> io::Result<()> {
    excluded
        .lock()
        .map_err(|_| io::Error::other("poisoned"))?
        .extend(hosted.iter().map(|&local| plan.suites[members[local]].path.clone()));
    crate::stamp::write_atomic(&kernel.with_extension("dill.failed"), b"")
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

/// Place every suite (absolute paths) by its import closure. The Flutter
/// SDK's own packages are the framework every suite runs on either way
/// and are not entered; the package's own code and its hosted
/// dependencies are.
///
/// # Errors
///
/// I/O failures reading the package configuration.
pub fn place(root: &Path, suites: &[PathBuf]) -> io::Result<Vec<Placement>> {
    use rayon::prelude::*;
    use rustc_hash::FxHashMap;

    let config_path = crate::cache::package_config_path(root);
    let text = std::fs::read_to_string(&config_path)?;
    let stock = || vec![Placement::Stock; suites.len()];
    let Ok(config) = dartq_pub::PackageConfig::parse(&text) else {
        return Ok(stock());
    };
    let Some(dart_tool) = config_path.parent() else {
        return Ok(stock());
    };
    let Ok(packages) = config.resolve(dart_tool) else {
        return Ok(stock());
    };
    let lib_dirs: FxHashMap<&str, &Path> =
        packages.iter().map(|package| (package.name.as_str(), package.lib_dir.as_path())).collect();
    let framework: Option<PathBuf> = config
        .additional_properties
        .get("flutterRoot")
        .and_then(serde_json::Value::as_str)
        .and_then(|uri| uri.strip_prefix("file://"))
        .map(PathBuf::from);
    let in_framework = |path: &Path| framework.as_deref().is_some_and(|sdk| path.starts_with(sdk));

    // Breadth-first over the closure, one frontier read in parallel.
    #[derive(Default)]
    struct Node {
        deps: Vec<u32>,
        launch: bool,
        shared: bool,
        untrusted: bool,
    }
    let mut ids: FxHashMap<PathBuf, u32> = FxHashMap::default();
    let mut nodes: Vec<Node> = Vec::new();
    let mut frontier: Vec<PathBuf> = Vec::new();
    for suite in suites {
        if !ids.contains_key(suite) {
            ids.insert(suite.clone(), u32::try_from(nodes.len()).unwrap_or(u32::MAX));
            nodes.push(Node::default());
            frontier.push(suite.clone());
        }
    }
    while !frontier.is_empty() {
        let read: Vec<(PathBuf, Option<(crate::deps::FileFacts, Option<Vec<PathBuf>>)>)> = frontier
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
                    .map(|deps| deps.into_iter().flatten().filter(|dep| !in_framework(dep)).collect());
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
            if to_stock[id] {
                Placement::Stock
            } else if to_fresh[id] {
                Placement::Fresh
            } else {
                Placement::Shared
            }
        })
        .collect())
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
        let sdk = flutter.canonicalize()?.parent().and_then(Path::parent).map(Path::to_path_buf).unwrap_or_default();
        for stamp in ["bin/cache/flutter.version.json", "bin/cache/engine.stamp", "bin/cache/dart-sdk/version"] {
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
        let bundle = root.join("build").join("unit_test_assets");
        for name in ["AssetManifest.bin", "FontManifest.json", "NativeAssetsManifest.json"] {
            let meta = std::fs::metadata(bundle.join(name)).ok();
            hasher.update(format!("{name}:{:?}:{:?}", meta.as_ref().map(std::fs::Metadata::len), meta.and_then(|meta| meta.modified().ok())).as_bytes());
        }
        let key = hasher.finalize().to_hex().to_string();
        let path = work_dir(root).join("recipe.json");
        #[derive(Serialize, Deserialize)]
        struct Stored {
            key: String,
            recipe: Recipe,
        }
        if let Some(stored) = std::fs::read(&path).ok().and_then(|bytes| serde_json::from_slice::<Stored>(&bytes).ok()) {
            if stored.key == key {
                return Ok(Some(stored.recipe));
            }
        }
        let Some(recipe) = Self::probe(flutter, root, args)? else {
            return Ok(None);
        };
        let bytes = serde_json::to_vec(&Stored { key, recipe: recipe.clone() }).map_err(io::Error::other)?;
        crate::stamp::write_atomic(&path, &bytes)?;
        Ok(Some(recipe))
    }
}

/// Any resolved package ships a `hook/build.dart`: the tool runs its
/// native-asset build before every run.
fn has_build_hooks(root: &Path) -> bool {
    let config_path = crate::cache::package_config_path(root);
    let Ok(text) = std::fs::read_to_string(&config_path) else { return true };
    let Ok(config) = dartq_pub::PackageConfig::parse(&text) else { return true };
    let Some(dart_tool) = config_path.parent() else { return true };
    let Ok(packages) = config.resolve(dart_tool) else { return true };
    packages.iter().any(|package| package.root.join("hook").join("build.dart").is_file())
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
        assert_eq!(recipe.compiler[0], "/sdk/bin/cache/dart-sdk/bin/dartaotruntime");
        // Per-run arguments are dartq's; everything else is the tool's.
        for dropped in ["--incremental", "--output-dill", "/t/output.dill", "--packages", "--initialize-from-dill", "/p/build/x.dill"] {
            assert!(!recipe.compiler.iter().any(|arg| arg == dropped), "{dropped}");
        }
        for kept in ["--target=flutter", "-DFLUTTER_VERSION=3.47.1", "--track-widget-creation", "--enable-experiment=alternative-invalidation-strategy"] {
            assert!(recipe.compiler.iter().any(|arg| arg == kept), "{kept}");
        }
        assert_eq!(recipe.tester.last().map(String::as_str), Some("--flutter-assets-dir=/p/build/unit_test_assets"));
        // The tester environment is what the probe observed, minus the
        // runner port and the fonts file dartq writes itself.
        assert_eq!(
            recipe.environment,
            [("APP_NAME", "p"), ("FLUTTER_TEST", "true"), ("HOME", "/h")].map(|(k, v)| (k.to_owned(), v.to_owned()))
        );
        assert!(recipe.fonts.contains("<dir>/sdk/bin/cache/artifacts</dir>"));
        // A tester argument the trace cannot split back is refused.
        assert!(Recipe::parse(&TRACE.replace("--non-interactive", "a, b")).is_none());
        assert!(Recipe::parse(&TRACE.replace("dartq.environment", "other")).is_none());
    }

    #[test]
    fn chunk_entries_wire_each_suite_like_the_stock_bootstrap() {
        let plain = Hosted { path: PathBuf::from("/p/test/a b_test.dart"), config: None, fresh: false };
        let wrapped = Hosted {
            path: PathBuf::from("/p/test/w/x_test.dart"),
            config: Some(PathBuf::from("/p/test/flutter_test_config.dart")),
            fresh: false,
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
        write("lib/script.dart", "import 'dart:io';\nvar s = Platform.script;");
        let plain = write("test/a_test.dart", "import 'package:p/missing.dart';\nvoid main() {}");
        let fresh = write("test/b_test.dart", "import 'package:p/via.dart';\nvoid main() {}");
        let stock = write("test/c_test.dart", "import '../lib/script.dart';\nvoid main() {}");
        let opaque = write("test/d_test.dart", "import 'x$y.dart';\nvoid main() {}");
        assert_eq!(
            place(&root, &[plain, fresh, stock, opaque]).expect("placed"),
            [Placement::Shared, Placement::Fresh, Placement::Stock, Placement::Stock]
        );
    }
}
