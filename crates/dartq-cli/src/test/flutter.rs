//! `dartq test --flutter` — `flutter test`, faster.
//!
//! The command line is `flutter test`'s own (`commands/test.dart`), and the
//! output is what `flutter test` prints for it. Suites run under the stock
//! tool — every suite is compiled by its `frontend_server`, started in its
//! own `flutter_tester` and reported by its `package:test` engine, with the
//! tool's asset bundle, fonts, golden comparator and
//! `flutter_test_config.dart` wrapping — so a suite's verdict is the tool's
//! verdict. dartq decides which suites run and where:
//!
//! - **Result cache.** A suite whose import closure, `flutter_test_config`,
//!   bundled assets, golden files, resolution, toolchain (the Flutter SDK
//!   and engine stamps), arguments and environment hash to the key of an
//!   earlier green run replays that run's events (see `dartq_test::cache`).
//! - **Grouped pass.** The suites still to run are compiled in shared
//!   chunks and run as spawned engines of reused `flutter_tester`
//!   processes under the stock runner (see `dartq_test::flutter`). A suite
//!   that ends green with nothing printed outside the protocol is final;
//!   every other suite reruns in the shards below, which report it.
//! - **Shards.** The suites still to run are split across several `flutter
//!   test` processes, each leased from the host-wide memory pool: one tool
//!   compiles its suites through one serial `frontend_server`, so on a
//!   multi-core host the compiler, not the tests, bounds a large run.
//!   Shards are balanced on the suites' recorded durations (longest first).
//! - **Resolution once.** The tool resolves packages before every run;
//!   dartq runs that check once (the first shard) and the rest with
//!   `--no-pub`, so parallel shards never contend on `pubspec.lock`.
//!
//! The merged event stream is shaped like one `flutter test --reporter
//! json` run and rendered with the reporter asked for. Anything the tool
//! alone reproduces runs as plain `flutter test` with the same arguments:
//! debugging and coverage, integration tests, web, devices, a random test
//! order, `--fail-fast`, `--experimental-faster-testing`, and malformed
//! arguments.

use std::borrow::Cow;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use rayon::prelude::*;
use rustc_hash::{FxHashMap, FxHashSet};

use dartq_test::event::Json;
use dartq_test::report::{self, Kind};
use dartq_test::{ExecConfig, Settled, SuiteBlock};

use super::{
    Invocation, NO_TESTS, Outputs, Sink, Tally, complete, delegate_requested, delegate_with, idle_timeout, tally,
    terminal_width,
};
use crate::exit;

/// The tool's directory for widget-test suites run as integration tests:
/// those run on a device, which only the tool drives.
const INTEGRATION_TEST_DIR: &str = "integration_test";

/// One suite: the absolute path the tool reports it under.
struct Suite {
    abs: PathBuf,
}

/// Run `flutter test` for `args`.
pub(super) fn run(args: &[String], inv: Invocation) -> u8 {
    let root = match std::env::current_dir().and_then(|dir| dir.canonicalize()) {
        Ok(root) => root,
        Err(error) => {
            eprintln!("dartq: cannot resolve the current directory: {error}");
            return exit::INTERNAL_ERROR;
        }
    };
    let Some(flutter) = dartq_test::host::find("flutter") else {
        return delegate_with(args, true, "no flutter on PATH");
    };
    // The tool's own preconditions (and their error text) are its to
    // report, as are setups only it can run.
    if inv.delegate {
        return delegate_requested(args, true, inv.stats, "tool-only options requested");
    }
    // Color is the tool's global option: its effect on the runner depends
    // on the tool's terminal, which only the tool reproduces.
    if inv.color.is_some() {
        return delegate_requested(args, true, inv.stats, "color option requested");
    }
    if let Some(reason) = state_reason(&root, super::global_config_exists()) {
        return delegate_with(args, true, &reason);
    }
    let Some(suites) = discover(&root, &inv.paths) else {
        return delegate_with(args, true, "no test suites discovered");
    };
    if suites.iter().any(|suite| suite.abs.starts_with(root.join(INTEGRATION_TEST_DIR))) {
        return delegate_with(args, true, "integration_test suites present");
    }
    let mut suites = suites;
    if let Some(base) = &inv.impacted {
        match dartq_commands::impact::impacted_tests(&root, None, base) {
            Ok(reached) => {
                let reached: FxHashSet<PathBuf> = reached.iter().map(|rel| root.join(rel)).collect();
                suites.retain(|suite| reached.contains(&suite.abs));
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
    // Suites the tool never loads (an excluded `@Tags`, a `@TestOn` the
    // tester platform fails) report nothing. The JSON reporter announces
    // the suite count before it knows, so a JSON run whose filtering is
    // uncertain belongs to the tool.
    let Some(filter) = dartq_test::select::TagFilter::new(&inv.include_tags, &inv.exclude_tags) else {
        return delegate_requested(args, true, inv.stats, "invalid tag selector");
    };
    let files: Vec<PathBuf> = suites.iter().map(|suite| suite.abs.clone()).collect();
    let selections = dartq_test::select::select_all(&files, &filter);
    let json = inv.machine || inv.reporter == Some(Kind::Json) || matches!(inv.file_reporter, Some((Kind::Json, _)));
    if json && selections.contains(&dartq_test::select::Selection::Unknown) {
        return delegate_requested(args, true, inv.stats, "json reporter with an uncertain selection");
    }
    let selected = suites.len();
    let mut selections = selections.into_iter();
    suites.retain(|_| selections.next() != Some(dartq_test::select::Selection::Dropped));
    // The tool resolves packages before running unless they are up to date
    // (and prints what it did); only an up-to-date resolution is
    // reproduced here, so no shard ever resolves.
    if !inv.no_pub {
        if let Some(reason) = stale_resolution(&root, &flutter) {
            return delegate_with(args, true, reason);
        }
    }
    let workers = inv.concurrency.unwrap_or_else(|| {
        // The tool's default: half the cores, at least one.
        std::thread::available_parallelism().map_or(1, |cores| (cores.get() / 2).max(1))
    });
    let plan = Plan { root, flutter, inv, suites, selected, workers };
    match execute(&plan) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("dartq: internal error: {error}");
            exit::INTERNAL_ERROR
        }
    }
}

/// Why the package's state sends the run to the tool, when it does: the
/// tool's own preconditions, named in the gate's own order. `global_config`
/// is `super::global_config_exists()`, passed in so the mapping stays pure.
fn state_reason(root: &Path, global_config: bool) -> Option<Cow<'static, str>> {
    if !root.join("pubspec.yaml").is_file() {
        return Some("no pubspec.yaml here".into());
    }
    let config = dartq_test::cache::package_config_path(root);
    if !config.is_file() {
        return Some(format!("no resolved {} — run flutter pub get", config.display()).into());
    }
    if root.join("dart_test.yaml").is_file() {
        return Some("dart_test.yaml present".into());
    }
    global_config.then(|| "global dart_test config present".into())
}

struct Plan {
    root: PathBuf,
    flutter: PathBuf,
    inv: Invocation,
    suites: Vec<Suite>,
    /// Suite files handed to the runner (before suite-level filtering).
    selected: usize,
    workers: usize,
}

/// The tool's suite list (`TestCommand.verifyThenRunCommand`): each
/// positional is a directory (every `*_test.dart` file beneath it, links
/// not followed, in listing order) or a file; with none, `test/`. Paths
/// are absolute and normalized, as the tool reports them. `None` when the
/// tool would report an error itself (no `test/` directory, no suites in
/// it) or a selection carries a query (`?name=`): the caller delegates.
fn discover(root: &Path, selections: &[String]) -> Option<Vec<Suite>> {
    let mut out: Vec<Suite> = Vec::new();
    let mut seen: FxHashSet<PathBuf> = FxHashSet::default();
    let mut push = |abs: PathBuf, out: &mut Vec<Suite>| {
        // The tool collects suites into a set of URIs.
        if seen.insert(abs.clone()) {
            out.push(Suite { abs });
        }
    };
    if selections.is_empty() {
        let dir = root.join("test");
        if !dir.is_dir() {
            return None;
        }
        let mut found = Vec::new();
        walk(&dir, &mut found).ok()?;
        if found.is_empty() {
            return None;
        }
        for abs in found {
            push(abs, &mut out);
        }
        return Some(out);
    }
    for selection in selections {
        if selection.contains(['?', '#', '%']) {
            return None;
        }
        let path = if Path::new(selection).is_absolute() { PathBuf::from(selection) } else { root.join(selection) };
        let path = dartq_source::normalize_lexical(&path);
        if path.is_dir() {
            let mut found = Vec::new();
            walk(&path, &mut found).ok()?;
            for abs in found {
                push(abs, &mut out);
            }
        } else {
            // A missing file is the tool's failed load to report.
            if !path.is_file() {
                return None;
            }
            push(path, &mut out);
        }
    }
    Some(out)
}

/// `Directory.listSync(recursive: true, followLinks: false)`, keeping
/// files named `*_test.dart`: entries in listing order, a directory's
/// entries right after it.
fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let path = entry.path();
        if kind.is_dir() {
            walk(&path, out)?;
        } else if path.to_string_lossy().ends_with("_test.dart") && std::fs::metadata(&path).is_ok_and(|meta| meta.is_file()) {
            out.push(path);
        }
    }
    Ok(())
}

/// How each suite was answered, for `--stats`.
#[derive(Default)]
struct Stats {
    replayed: usize,
    grouped: usize,
    ran: usize,
}


/// Run the plan: replay what the cache proves, run the rest in leased
/// `flutter test` shards, and emit every suite's events in selection order
/// as soon as every suite before it settled.
fn execute(plan: &Plan) -> io::Result<u8> {
    let started = Instant::now();
    let root = &plan.root;
    // The reporter runs in the tool's process: it is told `--no-color`
    // unless the tool's terminal supports color, and otherwise colors (and
    // picks `compact`) as `package:test` decides for that terminal.
    let special = report::color_default();
    let color = special && tool_supports_color();
    let args = plan.inv.pass.clone();
    let runner = dartq_test::Runner { bin: &plan.flutter, args: &args, flutter: true };

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

    // Every suite path is absolute, so the tool always prints it.
    let opts = report::Options {
        color,
        // The tool hands the runner one selection per suite file.
        print_path: plan.selected > 1,
        line_length: terminal_width(),
        executable: flutter_dart(&plan.flutter).display().to_string(),
        chain_stack_traces: true,
    };
    let kind = if plan.inv.machine {
        Kind::Json
    } else {
        plan.inv.reporter.unwrap_or_else(|| Kind::default_for(special))
    };
    let stdout: Box<dyn Write> = Box::new(BufWriter::new(io::stdout()));
    let mut outputs = Outputs {
        main: Sink::new(kind, &opts, stdout, None, started, plan.suites.len())?,
        file: match &plan.inv.file_reporter {
            Some((kind, path)) => {
                let path = root.join(path);
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir)?;
                }
                let file: Box<dyn Write> = Box::new(BufWriter::new(std::fs::File::create(&path)?));
                let file_opts = report::Options { color: false, ..opts.clone() };
                Some(Sink::file(*kind, &file_opts, file, None, started, plan.suites.len())?)
            }
            None => None,
        },
    };
    // The tool pins an older `package:test` engine: its GitHub reporter
    // groups skipped tests in place; `--machine` adds device events.
    outputs.each(|sink| {
        sink.legacy_github();
        Ok(())
    })?;
    if plan.inv.machine {
        outputs.main.machine();
    }

    let mut slots: Vec<Option<Vec<Json>>> = vec![None; plan.suites.len()];
    let mut emitted = 0;
    let mut next_id = 0u64;
    let mut clock = 0u64;
    let mut totals = Tally::default();
    let mut stats = Stats::default();
    let mut to_run: Vec<usize> = Vec::new();
    // A warm run parses every replayed entry's events: thousands of files,
    // each parsed on its own core.
    let replayed: Vec<Option<Vec<Json>>> = keys
        .par_iter()
        .map(|key| key.as_deref().and_then(|key| cache.as_ref().and_then(|cache| cache.lookup(key))))
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
    let mut emit = |slots: &mut Vec<Option<Vec<Json>>>, outputs: &mut Outputs, totals: &mut Tally| -> io::Result<()> {
        while emitted < slots.len() {
            let Some(mut events) = slots[emitted].take() else { break };
            if events.is_empty() {
                emitted += 1;
                continue;
            }
            let first = events.iter().find_map(|event| event.u64_at(&["time"])).unwrap_or(0);
            let now = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX).max(clock);
            report::rebase(&mut events, &mut next_id, now.saturating_sub(first));
            clock = events.iter().filter_map(|event| event.u64_at(&["time"])).max().unwrap_or(clock).max(clock);
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

    let mut staged: FxHashMap<usize, String> = FxHashMap::default();
    let mut tool_stderr = String::new();
    // The grouped pass first: suites it proves green with nothing printed
    // are final; every other suite runs under the stock tool below.
    if to_run.len() > 1 {
        let mut stock: Vec<usize> = Vec::new();
        let grouped = grouped_pass(plan, &args, &to_run, &mut stock, &mut |index, block| {
            stats.grouped += 1;
            if let (Some(cache), Some(key)) = (cache.as_mut(), keys[index].as_deref()) {
                cache.stage(key, &block.events)?;
                staged.insert(index, key.to_owned());
            }
            slots[index] = Some(block.events);
            emit(&mut slots, &mut outputs, &mut totals)
        })?;
        if grouped {
            to_run = stock;
        }
    }
    if !to_run.is_empty() {
        let index_of: FxHashMap<&Path, usize> =
            plan.suites.iter().enumerate().map(|(index, suite)| (suite.abs.as_path(), index)).collect();
        let outcome = run_shards(plan, &args, &to_run, &index_of, &mut |index, block| {
            stats.ran += 1;
            if complete(&block) && tally(&block.events).green {
                if let (Some(cache), Some(key)) = (cache.as_mut(), keys[index].as_deref()) {
                    cache.stage(key, &block.events)?;
                    staged.insert(index, key.to_owned());
                }
            }
            slots[index] = Some(block.events);
            emit(&mut slots, &mut outputs, &mut totals)
        })?;
        for (index, events) in outcome.lost {
            stats.ran += 1;
            slots[index] = Some(events);
        }
        emit(&mut slots, &mut outputs, &mut totals)?;
        for event in &outcome.loose {
            if event.kind() == "error" {
                totals.failed += 1;
            }
            outputs.each(|sink| sink.loose(event))?;
        }
        tool_stderr = outcome.stderr;
    }

    if let Some(cache) = cache.as_mut() {
        let indices: Vec<usize> = staged.keys().copied().collect();
        let files: Vec<PathBuf> = indices.iter().map(|&index| plan.suites[index].abs.clone()).collect();
        let mut verified: FxHashSet<String> = FxHashSet::default();
        match cache.rekey(&files, runner) {
            Ok(after) => {
                for (index, after) in indices.iter().zip(after) {
                    if after.as_deref() == Some(staged[index].as_str()) {
                        verified.insert(staged[index].clone());
                    }
                }
            }
            Err(error) => eprintln!("dartq test: result cache not updated ({error})"),
        }
        if let Err(error) = cache.commit(&verified).and_then(|()| cache.save()) {
            eprintln!("dartq test: result cache not saved ({error})");
        }
    }

    // A test that never finished (a suite that called `exit()` mid-test)
    // fails the run without joining the count.
    let success = totals.failed == 0 && !totals.incomplete;
    outputs.each(|sink| sink.done(success))?;
    eprint!("{tool_stderr}");
    if plan.inv.stats {
        eprintln!(
            "dartq test: {} suites — {} replayed, {} grouped, {} stock ({:.1}s)",
            plan.suites.len(),
            stats.replayed,
            stats.grouped,
            stats.ran,
            started.elapsed().as_secs_f64()
        );
    }
    if totals.passed + totals.skipped + totals.failed == 0 && !totals.incomplete {
        if tool_stderr.is_empty() {
            eprintln!("{}", super::no_tests_message(&plan.inv));
        }
        return Ok(NO_TESTS);
    }
    Ok(if success { exit::CLEAN } else { 1 })
}

/// The tool's color decision (`AnsiTerminal.supportsColor`): `NO_COLOR`
/// unset and standard output a terminal.
fn tool_supports_color() -> bool {
    if std::env::var_os("NO_COLOR").is_some() {
        return false;
    }
    #[cfg(unix)]
    {
        // SAFETY: plain query on the process's own standard output.
        unsafe { libc::isatty(libc::STDOUT_FILENO) == 1 }
    }
    #[cfg(not(unix))]
    {
        false
    }
}

/// The VM the tool's `Platform.executable` names (the compact reporter's
/// rerun hint): the Dart SDK inside the Flutter SDK's cache.
fn flutter_dart(flutter: &Path) -> PathBuf {
    let resolved = flutter.canonicalize().unwrap_or_else(|_| flutter.to_path_buf());
    resolved
        .parent()
        .map_or_else(|| PathBuf::from("dart"), |bin| bin.join("cache").join("dart-sdk").join("bin").join("dart"))
}

/// Why the tool would resolve packages before running (`Pub.get`), when it
/// would: `None` when the resolution is current, `Some` naming the
/// conjunct that failed. The tool's own test
/// (`Pub.get(checkUpToDate: true)`): a package configuration not written
/// by pub is left alone; otherwise the pubspec is older than both the lock
/// and the package configuration, and the configuration was written by
/// this Flutter version (`.dart_tool/version`). Workspace members resolve
/// at the workspace root.
fn stale_resolution(root: &Path, flutter: &Path) -> Option<&'static str> {
    let config = dartq_test::cache::package_config_path(root);
    let Some(workspace) = config.parent().and_then(Path::parent) else {
        return Some("no package configuration — run flutter pub get");
    };
    let generator = std::fs::read_to_string(&config)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|value| value.get("generator").and_then(|generator| generator.as_str().map(str::to_owned)));
    if generator.as_deref().is_some_and(|generator| generator != "pub") {
        // Another generator's configuration is left alone: current.
        return None;
    }
    let modified = |path: &Path| std::fs::metadata(path).and_then(|meta| meta.modified()).ok();
    let (Some(pubspec), Some(lock), Some(config_time)) =
        (modified(&root.join("pubspec.yaml")), modified(&workspace.join("pubspec.lock")), modified(&config))
    else {
        return Some("resolution inputs missing — run flutter pub get");
    };
    let recorded = std::fs::read_to_string(workspace.join(".dart_tool").join("version")).ok();
    let Some(recorded) = recorded else {
        return Some("no Flutter resolution stamp — run flutter pub get");
    };
    if !framework_version(flutter).is_some_and(|version| version == recorded) {
        return Some("the resolution is from another Flutter — run flutter pub get");
    }
    if pubspec < lock && pubspec < config_time {
        None
    } else {
        Some("pubspec newer than lock/config — resolution stale (run flutter pub get)")
    }
}

/// The Flutter SDK's framework version (`bin/cache/flutter.version.json`).
fn framework_version(flutter: &Path) -> Option<String> {
    let resolved = flutter.canonicalize().ok()?;
    let text = std::fs::read_to_string(resolved.parent()?.join("cache").join("flutter.version.json")).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    Some(value.get("frameworkVersion")?.as_str()?.to_owned())
}

/// What the shards left over once every suite they reported settled.
struct Leftover {
    /// The final events of each suite no shard reported: none when its
    /// shard finished (the tool filtered the suite out), else the failed
    /// load the tool reports for a suite whose runner died.
    lost: Vec<(usize, Vec<Json>)>,
    /// Events naming no suite.
    loose: Vec<Json>,
    /// What the tools wrote to standard error.
    stderr: String,
}

/// Run `indices` across leased `flutter test` shards. Each suite's block
/// reaches `on_block` as the suite settles.
fn run_shards(
    plan: &Plan,
    args: &[String],
    indices: &[usize],
    index_of: &FxHashMap<&Path, usize>,
    on_block: &mut dyn FnMut(usize, SuiteBlock) -> io::Result<()>,
) -> io::Result<Leftover> {
    let root = &plan.root;
    let profile = dartq_test::Profile::FLUTTER.calibrated("flutter");
    let target = plan.workers.min(indices.len()).max(1);
    // One tool compiles its suites through one serial `frontend_server`:
    // split the concurrency into processes of up to SUITES_PER_SHARD each.
    let wanted = target.div_ceil(SUITES_PER_SHARD);
    let deadline = Instant::now() + idle_timeout();
    let (lease, workers) = dartq_test::lease_processes(profile, target, wanted, deadline)?
        .ok_or_else(|| io::Error::other("timed out waiting for host memory held by other dartq runs"))?;
    let (shards, workers) = match &lease {
        Some(lease) => dartq_test::split(profile, lease.units(), target, wanted),
        None => (wanted.min(workers).max(1), workers),
    };
    let per_shard = workers.div_ceil(shards).max(1);
    let durations = dartq_test::group::Durations::load(root);
    let files: Vec<PathBuf> = indices.iter().map(|&index| plan.suites[index].abs.clone()).collect();
    let parts = dartq_test::group::shard(&files, shards, |suite| durations.get(root, suite));
    let shard_of: FxHashMap<&Path, usize> =
        parts.iter().enumerate().flat_map(|(shard, part)| part.iter().map(move |abs| (abs.as_path(), shard))).collect();

    let (sender, receiver) = std::sync::mpsc::channel::<dartq_test::Streamed>();
    let mut settled: FxHashSet<usize> = FxHashSet::default();
    let mut recorded: Vec<(PathBuf, u64)> = Vec::new();
    let mut failure: Option<io::Error> = None;
    let mut results: Vec<Option<Result<dartq_test::Execution, dartq_test::ExecError>>> = Vec::new();
    // Every tool prepares the package's shared `build/unit_test_assets`
    // and `build/native_assets` before loading suites — clearing the
    // native-asset directory, then writing and copying its manifest — so
    // two tools preparing at once can delete each other's files. Each shard
    // starts once the one before it reported a suite (preparation done; the
    // next finds the assets current), or ended without one.
    std::thread::scope(|scope| {
        let mut previous: Option<std::sync::mpsc::Receiver<()>> = None;
        let handles: Vec<_> = parts
            .iter()
            .map(|part| {
                if let Some(previous) = previous.take() {
                    let _ = previous.recv();
                }
                let (ready, prepared) = std::sync::mpsc::channel::<()>();
                previous = Some(prepared);
                let cfg = ExecConfig {
                    dart_bin: plan.flutter.clone(),
                    cwd: root.clone(),
                    extra_args: ["--no-pub".to_owned(), format!("--concurrency={per_shard}")]
                        .into_iter()
                        .chain(args.iter().cloned())
                        .collect(),
                    timeout: Some(idle_timeout()),
                    sink: Some(sender.clone()),
                    ready: Some(ready),
                };
                scope.spawn(move || dartq_test::execute_flutter(part, &cfg))
            })
            .collect();
        drop(sender);
        for streamed in receiver {
            // A raw line of no suite in flight prints where the tool wrote
            // it; every other item is a settled suite's block.
            let dartq_test::Streamed::Settled(Settled { block, .. }) = streamed else {
                if let dartq_test::Streamed::Line(line) = streamed {
                    println!("{line}");
                }
                continue;
            };
            if failure.is_some() {
                continue;
            }
            let Some(index) = index_of.get(Path::new(&block.path)).copied() else { continue };
            if let Some(ms) = block_duration(&block.events) {
                recorded.push((plan.suites[index].abs.strip_prefix(root).unwrap_or(&plan.suites[index].abs).to_path_buf(), ms));
            }
            settled.insert(index);
            if let Err(error) = on_block(index, block) {
                failure = Some(error);
            }
        }
        results = handles.into_iter().map(|handle| handle.join().ok()).collect();
    });
    drop(lease);
    let peak: u64 = results.iter().flatten().flatten().map(|execution| execution.peak_rss).sum();
    profile.observe("flutter", peak, workers, parts.len());
    durations.record(root, &recorded);
    if let Some(error) = failure {
        return Err(error);
    }
    let mut leftover = Leftover { lost: Vec::new(), loose: Vec::new(), stderr: String::new() };
    let mut finished = vec![false; parts.len()];
    for (shard, result) in results.into_iter().enumerate() {
        match result {
            Some(Ok(execution)) => {
                finished[shard] = execution.complete;
                for loose in execution.loose {
                    match loose {
                        dartq_test::Loose::Event(event) => leftover.loose.push(event),
                        // The tool's own stdout lines (device events, tester
                        // relays) print as the tool printed them.
                        dartq_test::Loose::Line(line) => println!("{line}"),
                    }
                }
                // Shards start together and wait their turn on the tool's
                // startup lock; that wait is dartq's, not the run's.
                for line in execution.stderr.split_inclusive('\n') {
                    if line.trim_end() != STARTUP_LOCK {
                        leftover.stderr.push_str(line);
                    }
                }
            }
            Some(Err(error)) => eprintln!("dartq test: flutter test failed: {error}"),
            None => eprintln!("dartq test: a flutter test shard panicked"),
        }
    }
    for &index in indices.iter().filter(|index| !settled.contains(index)) {
        let abs = plan.suites[index].abs.as_path();
        let events = if finished[shard_of[abs]] { Vec::new() } else { super::lost_suite(&abs.to_string_lossy()) };
        leftover.lost.push((index, events));
    }
    Ok(leftover)
}

/// The `package:test` arguments the tool hands its runner for this
/// invocation (`FlutterTestRunner.runTests`'s `testArgs`), before `--`.
/// `None` when an argument shapes the run in a way the grouped pass does
/// not reproduce (`--update-goldens` writes files; `--test-assets=false`
/// changes what the tester loads).
fn runner_args(pass: &[String], color: bool) -> Option<Vec<String>> {
    let mut out = Vec::new();
    if !color {
        out.push("--no-color".to_owned());
    }
    let value = |name: &str| -> Vec<String> {
        let prefix = format!("--{name}=");
        pass.iter().filter_map(|token| token.strip_prefix(&prefix).map(str::to_owned)).collect()
    };
    let flag = |name: &str| pass.iter().any(|token| token == &format!("--{name}"));
    if let Some(timeout) = value("timeout").pop() {
        out.extend(["--timeout".to_owned(), timeout]);
    }
    if flag("ignore-timeouts") {
        out.push("--ignore-timeouts".to_owned());
    }
    for name in value("name") {
        out.extend(["--name".to_owned(), name]);
    }
    for name in value("plain-name") {
        out.extend(["--plain-name".to_owned(), name]);
    }
    // The tool takes the last `--tags`/`--exclude-tags` (single options).
    if let Some(tags) = value("tags").pop() {
        out.extend(["--tags".to_owned(), tags]);
    }
    if let Some(tags) = value("exclude-tags").pop() {
        out.extend(["--exclude-tags".to_owned(), tags]);
    }
    if flag("run-skipped") {
        out.push("--run-skipped".to_owned());
    }
    if let Some(total) = value("total-shards").pop() {
        out.push(format!("--total-shards={total}"));
    }
    if let Some(index) = value("shard-index").pop() {
        out.push(format!("--shard-index={index}"));
    }
    out.push("--chain-stack-traces".to_owned());
    if flag("update-goldens") || flag("no-test-assets") {
        return None;
    }
    Some(out)
}

/// The tool arguments that shape the compile or the tester (defines,
/// experiments, flavor, rendering flags), handed to [`dartq_test::flutter::Recipe`].
fn build_args(pass: &[String]) -> Vec<String> {
    const SHAPING: &[&str] = &[
        "--dart-define=",
        "--dart-define-from-file=",
        "--enable-experiment=",
        "--flavor=",
        "--track-widget-creation",
        "--no-track-widget-creation",
        "--enable-impeller",
        "--no-enable-impeller",
        "--enable-flutter-gpu",
        "--no-enable-flutter-gpu",
        "--enable-hcpp",
        "--no-enable-hcpp",
    ];
    pass.iter().filter(|token| SHAPING.iter().any(|shape| token.starts_with(shape))).cloned().collect()
}

/// Run `indices` through the grouped pass. Each suite proven green with
/// nothing printed reaches `on_block`; every other suite lands in `stock`.
/// `false` (with `stock` untouched) when the pass cannot run at all: the
/// caller then runs every suite stock.
fn grouped_pass(
    plan: &Plan,
    args: &[String],
    indices: &[usize],
    stock: &mut Vec<usize>,
    on_block: &mut dyn FnMut(usize, SuiteBlock) -> io::Result<()>,
) -> io::Result<bool> {
    use dartq_test::flutter::{Hosted, Placement};
    let root = &plan.root;
    if std::env::var_os("DARTQ_TEST_NO_GROUP").is_some() {
        return Ok(false);
    }
    let Some(runner_args) = runner_args(args, tool_supports_color() && report::color_default()) else {
        return Ok(false);
    };
    let Ok(flutter) = plan.flutter.canonicalize() else { return Ok(false) };
    let Some(sdk) = flutter.parent().and_then(Path::parent) else { return Ok(false) };
    let vm = sdk.join("bin").join("cache").join("dart-sdk").join("bin").join("dart");
    let tool_packages = sdk.join("packages").join("flutter_tools").join(".dart_tool").join("package_config.json");
    if !vm.is_file() || !tool_packages.is_file() {
        return Ok(false);
    }
    let Some(recipe) = dartq_test::flutter::Recipe::load(&flutter, root, &build_args(args))? else {
        return Ok(false);
    };
    let files: Vec<PathBuf> = indices.iter().map(|&index| plan.suites[index].abs.clone()).collect();
    let placements = dartq_test::flutter::place(root, &files)?;
    let mut hosted: Vec<Hosted> = Vec::new();
    let mut hosted_index: Vec<usize> = Vec::new();
    for ((&index, file), placement) in indices.iter().zip(&files).zip(&placements) {
        if *placement == Placement::Stock {
            stock.push(index);
            continue;
        }
        let (configs, _) = dartq_test::cache::flutter_inputs(root, file);
        hosted.push(Hosted { path: file.clone(), config: configs.into_iter().next(), fresh: *placement == Placement::Fresh });
        hosted_index.push(index);
    }
    if hosted.len() < 2 {
        stock.extend(hosted_index);
        return Ok(true);
    }
    // Shared suites in the selection's order, fresh ones last: their own
    // testers start once the shared chunks run.
    let mut order: Vec<usize> = (0..hosted.len()).collect();
    order.sort_by_key(|&i| hosted[i].fresh);
    let hosted: Vec<Hosted> = order.iter().map(|&i| hosted[i].clone()).collect();
    let hosted_index: Vec<usize> = order.iter().map(|&i| hosted_index[i]).collect();

    let profile = dartq_test::Profile::FLUTTER_GROUPED.calibrated("flutter-grouped");
    let deadline = Instant::now() + idle_timeout();
    let target = plan.workers.min(hosted.len()).max(1);
    let (lease, workers) = dartq_test::lease(profile, target, deadline)?
        .ok_or_else(|| io::Error::other("timed out waiting for host memory held by other dartq runs"))?;
    let index_of: FxHashMap<&Path, usize> =
        hosted.iter().zip(&hosted_index).map(|(suite, &index)| (suite.path.as_path(), index)).collect();
    let (sender, receiver) = std::sync::mpsc::channel::<dartq_test::Streamed>();
    let cfg = ExecConfig {
        dart_bin: PathBuf::new(),
        cwd: root.clone(),
        extra_args: Vec::new(),
        timeout: Some(idle_timeout()),
        sink: Some(sender),
        ready: None,
    };
    let packages = dartq_test::cache::package_config_path(root);
    let run = dartq_test::flutter::Plan {
        root,
        vm: &vm,
        tool_packages: &tool_packages,
        packages: &packages,
        recipe: &recipe,
        suites: &hosted,
        runner_args: &runner_args,
        concurrency: workers,
    };
    let mut settled: FxHashSet<usize> = FxHashSet::default();
    let mut failure: Option<io::Error> = None;
    let mut recorded: Vec<(PathBuf, u64)> = Vec::new();
    let mut outcome = None;
    std::thread::scope(|scope| {
        let run = &run;
        let handle = scope.spawn(move || dartq_test::flutter::execute(run, &cfg));
        for streamed in &receiver {
            // A raw line of no suite in flight prints where the tool wrote
            // it; every other item is a settled suite's block.
            let dartq_test::Streamed::Settled(Settled { block, .. }) = streamed else {
                if let dartq_test::Streamed::Line(line) = streamed {
                    println!("{line}");
                }
                continue;
            };
            if failure.is_some() {
                continue;
            }
            let Some(&index) = index_of.get(Path::new(&block.path)) else { continue };
            if !(block.clean && complete(&block) && tally(&block.events).green) {
                continue;
            }
            if let Some(ms) = block_duration(&block.events) {
                let abs = &plan.suites[index].abs;
                recorded.push((abs.strip_prefix(root).unwrap_or(abs).to_path_buf(), ms));
            }
            settled.insert(index);
            if let Err(error) = on_block(index, block) {
                failure = Some(error);
            }
        }
        outcome = handle.join().ok();
    });
    drop(lease);
    if let Some(error) = failure {
        return Err(error);
    }
    if let Some(Ok(outcome)) = &outcome {
        profile.observe("flutter-grouped", outcome.execution.peak_rss, workers, 1);
    }
    dartq_test::group::Durations::load(root).record(root, &recorded);
    stock.extend(hosted_index.iter().copied().filter(|index| !settled.contains(index)));
    stock.sort_unstable();
    Ok(true)
}

/// The tool's notice while another tool holds the Flutter SDK's startup
/// lock (`Cache.lock`).
const STARTUP_LOCK: &str = "Waiting for another flutter command to release the startup lock...";

/// Concurrent suites per `flutter test` process. Measured on a 43-suite
/// widget-test package (10-core host): one process at `-j 5` 23.2 s /
/// 1.43 GB, at `-j 4` 25.4 s / 1.33 GB; four processes of two 11.1 s.
const SUITES_PER_SHARD: usize = 2;

/// The span between a block's first and last `time` stamp.
fn block_duration(events: &[Json]) -> Option<u64> {
    let first = events.iter().find_map(|event| event.u64_at(&["time"]))?;
    let last = events.iter().rev().find_map(|event| event.u64_at(&["time"]))?;
    Some(last.saturating_sub(first))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn discovery_matches_the_tool() {
        let tmp = tempfile::tempdir().expect("tmp");
        let root = tmp.path().canonicalize().expect("canonical");
        for rel in ["test/a_test.dart", "test/sub/b_test.dart", "test/helper.dart", "test/.hidden/c_test.dart", "other/d_test.dart"] {
            let path = root.join(rel);
            fs::create_dir_all(path.parent().expect("parent")).expect("dir");
            fs::write(&path, "void main() {}").expect("write");
        }
        // No selection: everything under `test/`, hidden directories
        // included, absolute.
        let mut all: Vec<PathBuf> = discover(&root, &[]).expect("found").into_iter().map(|suite| suite.abs).collect();
        all.sort();
        assert_eq!(
            all,
            ["test/.hidden/c_test.dart", "test/a_test.dart", "test/sub/b_test.dart"].map(|rel| root.join(rel)).to_vec()
        );
        // Selections: directories and files, normalized, each file once.
        let picked: Vec<PathBuf> = discover(&root, &["./test/sub/../a_test.dart".to_owned(), "other".to_owned(), "test/a_test.dart".to_owned()])
            .expect("found")
            .into_iter()
            .map(|suite| suite.abs)
            .collect();
        assert_eq!(picked, [root.join("test/a_test.dart"), root.join("other/d_test.dart")]);
        // The tool's own errors: a missing file, a selector query, an
        // empty test directory.
        assert!(discover(&root, &["test/missing_test.dart".to_owned()]).is_none());
        assert!(discover(&root, &["test/a_test.dart?name=x".to_owned()]).is_none());
        let empty = tempfile::tempdir().expect("tmp");
        fs::create_dir_all(empty.path().join("test")).expect("dir");
        assert!(discover(empty.path(), &[]).is_none());
    }

    #[test]
    fn pub_freshness_follows_the_tool() {
        let tmp = tempfile::tempdir().expect("tmp");
        let root = tmp.path();
        let sdk = root.join("sdk");
        fs::create_dir_all(sdk.join("bin/cache")).expect("sdk");
        fs::write(sdk.join("bin/flutter"), "").expect("flutter");
        fs::write(sdk.join("bin/cache/flutter.version.json"), r#"{"frameworkVersion":"3.0.0"}"#).expect("version");
        let flutter = sdk.join("bin/flutter");
        let pkg = root.join("pkg");
        fs::create_dir_all(pkg.join(".dart_tool")).expect("pkg");
        let at = |path: &Path, secs: u64| {
            filetime::set_file_mtime(path, filetime::FileTime::from_unix_time(i64::try_from(secs).expect("small"), 0)).expect("mtime");
        };
        fs::write(pkg.join("pubspec.yaml"), "name: p").expect("pubspec");
        fs::write(pkg.join("pubspec.lock"), "packages: {}").expect("lock");
        fs::write(pkg.join(".dart_tool/package_config.json"), r#"{"configVersion":2,"packages":[],"generator":"pub"}"#).expect("config");
        fs::write(pkg.join(".dart_tool/version"), "3.0.0").expect("stamp");
        at(&pkg.join("pubspec.yaml"), 100);
        at(&pkg.join("pubspec.lock"), 200);
        at(&pkg.join(".dart_tool/package_config.json"), 200);
        assert_eq!(stale_resolution(&pkg, &flutter), None);
        // A newer pubspec, or another Flutter version, means the tool
        // would resolve — and the reason names which conjunct failed.
        at(&pkg.join("pubspec.yaml"), 300);
        assert_eq!(
            stale_resolution(&pkg, &flutter),
            Some("pubspec newer than lock/config — resolution stale (run flutter pub get)")
        );
        at(&pkg.join("pubspec.yaml"), 100);
        fs::write(pkg.join(".dart_tool/version"), "2.9.0").expect("stamp");
        assert_eq!(stale_resolution(&pkg, &flutter), Some("the resolution is from another Flutter — run flutter pub get"));
        // A configuration another generator wrote is left alone.
        fs::write(pkg.join(".dart_tool/package_config.json"), r#"{"configVersion":2,"packages":[],"generator":"bazel"}"#).expect("config");
        assert_eq!(stale_resolution(&pkg, &flutter), None);
    }

    /// State delegations name the missing precondition, in the gate's own
    /// order: the pubspec, the resolved package configuration, the local
    /// `dart_test.yaml`, the global one.
    #[test]
    fn state_reasons_name_the_missing_precondition() {
        let tmp = tempfile::tempdir().expect("tmp");
        let root = tmp.path();
        assert_eq!(state_reason(root, false).as_deref(), Some("no pubspec.yaml here"));
        fs::write(root.join("pubspec.yaml"), "name: p\n").expect("write");
        let reason = state_reason(root, false).expect("stale");
        assert!(reason.contains("package_config.json"), "{reason}");
        assert!(reason.contains("run flutter pub get"), "{reason}");
        fs::create_dir_all(root.join(".dart_tool")).expect("dir");
        fs::write(root.join(".dart_tool/package_config.json"), r#"{"configVersion":2,"packages":[]}"#).expect("write");
        assert_eq!(state_reason(root, false), None, "a resolved, config-less package runs here");
        fs::write(root.join("dart_test.yaml"), "concurrency: 2\n").expect("write");
        assert_eq!(state_reason(root, false).as_deref(), Some("dart_test.yaml present"));
        fs::remove_file(root.join("dart_test.yaml")).expect("remove");
        assert_eq!(state_reason(root, true).as_deref(), Some("global dart_test config present"));
    }

    /// The `package:test` arguments the tool hands its runner: no color by
    /// default, the last `--timeout`/`--tags` value wins, names repeat,
    /// and the goldens and asset switches opt the grouped pass out.
    #[test]
    fn runner_args_match_the_tools_handoff() {
        let pass = [
            "--timeout=1x",
            "--timeout=2x",
            "--name=a",
            "--name=b",
            "--plain-name=p",
            "--tags=s",
            "--tags=t",
            "--exclude-tags=e",
            "--exclude-tags=f",
            "--run-skipped",
            "--ignore-timeouts",
            "--total-shards=4",
            "--shard-index=2",
        ]
        .map(str::to_owned);
        assert_eq!(
            runner_args(&pass, false).expect("args"),
            [
                "--no-color",
                "--timeout",
                "2x",
                "--ignore-timeouts",
                "--name",
                "a",
                "--name",
                "b",
                "--plain-name",
                "p",
                "--tags",
                "t",
                "--exclude-tags",
                "f",
                "--run-skipped",
                "--total-shards=4",
                "--shard-index=2",
                "--chain-stack-traces",
            ]
        );
        let colored = runner_args(&pass, true).expect("args");
        assert_eq!(colored.first().map(String::as_str), Some("--timeout"));
        assert!(runner_args(&["--update-goldens".to_owned()], false).is_none());
        assert!(runner_args(&["--no-test-assets".to_owned()], false).is_none());
    }

    /// Only the arguments that shape the compile or the tester reach the
    /// recipe probe.
    #[test]
    fn build_args_pick_the_shaping_flags() {
        let pass = [
            "--dart-define=A=1",
            "--dart-define-from-file=env.json",
            "--enable-experiment=x",
            "--flavor=f",
            "--track-widget-creation",
            "--no-enable-impeller",
            "--enable-hcpp",
            "--timeout=1x",
            "--name=a",
        ]
        .map(str::to_owned);
        assert_eq!(
            build_args(&pass),
            [
                "--dart-define=A=1",
                "--dart-define-from-file=env.json",
                "--enable-experiment=x",
                "--flavor=f",
                "--track-widget-creation",
                "--no-enable-impeller",
                "--enable-hcpp",
            ]
            .map(str::to_owned)
            .to_vec()
        );
    }

    /// The compact hint names the Dart SDK inside the Flutter SDK's cache.
    #[test]
    fn flutter_hints_name_the_cached_sdk() {
        assert_eq!(
            flutter_dart(Path::new("/sdk/bin/flutter")),
            PathBuf::from("/sdk/bin/cache/dart-sdk/bin/dart")
        );
    }
}
