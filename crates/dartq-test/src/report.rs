//! `dart test`'s reporters, rendered from the JSON protocol.
//!
//! dartq runs suites through the runner's own JSON reporter (in several
//! processes, or not at all when a result replays from the cache) and
//! re-renders the merged event stream with the reporter the caller asked
//! for. Each renderer mirrors its `package:test` counterpart line for line
//! (`expanded`, `failures-only`, `github`, `compact`, `silent`, `json`),
//! driven by the events that counterpart listens to: test start, test
//! output (`print`), errors, test end, and the end of the run.

use crate::event::Json;
use std::io::{self, Write};
use std::time::Instant;

/// One reporter of the `package:test` set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Compact,
    Expanded,
    FailuresOnly,
    Github,
    Json,
    Silent,
}

impl Kind {
    /// The name `--reporter` takes.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "compact" => Self::Compact,
            "expanded" => Self::Expanded,
            "failures-only" => Self::FailuresOnly,
            "github" => Self::Github,
            "json" => Self::Json,
            "silent" => Self::Silent,
            _ => return None,
        })
    }

    /// The runner's default: `github` under GitHub Actions, `compact` when
    /// the output is a color terminal, `expanded` otherwise.
    #[must_use]
    pub fn default_for(color: bool) -> Self {
        if std::env::var("GITHUB_ACTIONS").as_deref() == Ok("true") {
            Self::Github
        } else if color {
            Self::Compact
        } else {
            Self::Expanded
        }
    }
}

/// Whether the runner would color its output: `NO_COLOR`, `TERM=dumb` and
/// `CLICOLOR=0` disable, `FORCE_COLOR`/`CLICOLOR_FORCE` enable, else a
/// terminal on standard output.
#[must_use]
pub fn color_default() -> bool {
    let set = |name: &str| std::env::var_os(name).is_some();
    if set("NO_COLOR") || std::env::var("TERM").as_deref() == Ok("dumb") || std::env::var("CLICOLOR").as_deref() == Ok("0") {
        return false;
    }
    if set("FORCE_COLOR") || set("CLICOLOR_FORCE") {
        return true;
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

/// What a renderer is told about the run up front.
#[derive(Debug, Clone)]
pub struct Options {
    pub color: bool,
    /// Prefix test names with their suite path: more than one selection,
    /// or a directory selection (the runner's own rule).
    pub print_path: bool,
    /// The terminal width the compact reporter pads and truncates to.
    pub line_length: usize,
    /// The runner's `Platform.executable` spelling for the compact
    /// reporter's rerun hint.
    pub executable: String,
    /// `--chain-stack-traces` is on: no notice suggesting it.
    pub chain_stack_traces: bool,
}

/// A test as the renderers track it.
#[derive(Debug, Clone)]
struct Test {
    name: String,
    suite: u64,
    /// The hidden load pseudo-test of a suite.
    load: bool,
    /// `setUpAll`/`tearDownAll` bookkeeping tests.
    synthetic: bool,
    skip: bool,
    done: bool,
    skipped: bool,
    hidden: bool,
    errors: Vec<(String, String)>,
    messages: Vec<(String, String)>,
    /// Counted as failed (at its first error, as the runner's state change
    /// precedes the error report).
    counted: bool,
    /// The runner's result once the test completed: `success`, `failure`
    /// or `error`.
    result: &'static str,
}

const GREEN: &str = "\u{1b}[32m";
const RED: &str = "\u{1b}[31m";
const YELLOW: &str = "\u{1b}[33m";
const GRAY: &str = "\u{1b}[90m";
const CYAN: &str = "\u{1b}[36m";
const BOLD: &str = "\u{1b}[1m";
const NO_COLOR: &str = "\u{1b}[0m";

/// A human reporter (`compact`, `expanded`, `failures-only`, `silent`)
/// driven by protocol events, one suite block at a time.
pub struct Renderer {
    kind: Kind,
    opts: Options,
    started: Instant,
    tests: rustc_hash::FxHashMap<u64, Test>,
    /// Tests an `error` event named: every real failure carries one before
    /// its `testDone`.
    errored: rustc_hash::FxHashSet<u64>,
    suites: rustc_hash::FxHashMap<u64, String>,
    passed: usize,
    skipped: usize,
    failed: usize,
    /// Failed or errored tests, for the closing summary (path, name).
    failures: Vec<(String, String)>,
    /// User tests seen (the runner's `liveTests`, load tests excluded).
    live: usize,
    last: Option<(usize, usize, usize, String, Option<String>)>,
    chain_notice: bool,
    /// Started, unfinished user tests in start order (the engine's
    /// `active` set).
    active: Vec<u64>,
}

impl Renderer {
    #[must_use]
    pub fn new(kind: Kind, opts: Options) -> Self {
        Self {
            kind,
            opts,
            started: Instant::now(),
            tests: rustc_hash::FxHashMap::default(),
            errored: rustc_hash::FxHashSet::default(),
            suites: rustc_hash::FxHashMap::default(),
            passed: 0,
            skipped: 0,
            failed: 0,
            failures: Vec::new(),
            live: 0,
            last: None,
            chain_notice: false,
            active: Vec::new(),
        }
    }

    fn paint(&self, code: &'static str) -> &'static str {
        if self.opts.color { code } else { "" }
    }

    fn description(&self, test: &Test) -> String {
        let mut name = test.name.clone();
        if self.opts.print_path && !test.load {
            if let Some(path) = self.suites.get(&test.suite) {
                name = format!("{path}: {name}");
            }
        }
        if test.load {
            name = format!("{}{}{name}{}", self.paint(BOLD), self.paint(GRAY), self.paint(NO_COLOR));
        }
        name
    }

    /// The `+P ~S -F: message` progress line, deduplicated as the runner
    /// does: unchanged counts, message and suffix print nothing.
    fn progress(&mut self, out: &mut dyn Write, message: &str, color: Option<&'static str>, suffix: Option<&str>) -> io::Result<()> {
        let state = (self.passed, self.skipped, self.failed, message.to_owned(), suffix.map(str::to_owned));
        if let Some(last) = &self.last {
            if last.0 == state.0 && last.1 == state.1 && last.2 == state.2 && last.3 == state.3 && (suffix.is_none() || last.4 == state.4) {
                return Ok(());
            }
        }
        self.last = Some(state);
        let mut line = String::new();
        if self.kind != Kind::FailuresOnly {
            let elapsed = self.started.elapsed().as_secs();
            line.push_str(&format!("{:02}:{:02} ", elapsed / 60, elapsed % 60));
        }
        line.push_str(&format!("{}+{}{}", self.paint(GREEN), self.passed, self.paint(NO_COLOR)));
        if self.skipped > 0 {
            line.push_str(&format!("{} ~{}{}", self.paint(YELLOW), self.skipped, self.paint(NO_COLOR)));
        }
        if self.failed > 0 {
            line.push_str(&format!("{} -{}{}", self.paint(RED), self.failed, self.paint(NO_COLOR)));
        }
        line.push_str(": ");
        line.push_str(color.map_or("", |c| self.paint(c)));
        line.push_str(message);
        if let Some(suffix) = suffix {
            line.push_str(suffix);
        }
        line.push_str(self.paint(NO_COLOR));
        writeln!(out, "{line}")
    }
}

/// `text` with ANSI color escapes removed.
fn strip_colors(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            for c in chars.by_ref() {
                if c == 'm' {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Two spaces before every line (the runner's `indent`).
fn indent(text: &str) -> String {
    text.split('\n').map(|line| format!("  {line}")).collect::<Vec<_>>().join("\n")
}

impl Renderer {
    /// Render one suite's events. Suites render one after another, so the
    /// output is what the runner prints for the same suites at
    /// `--concurrency=1`: a parallel run's interleaving is a scheduling
    /// accident, not part of the format.
    ///
    /// # Errors
    ///
    /// Write failures on `out`.
    pub fn block(&mut self, events: &[Json], out: &mut dyn Write) -> io::Result<()> {
        for event in events {
            self.event(event, out)?;
        }
        Ok(())
    }

    /// Render one event.
    ///
    /// # Errors
    ///
    /// Write failures on `out`.
    pub fn event(&mut self, event: &Json, out: &mut dyn Write) -> io::Result<()> {
        if self.kind == Kind::Silent || self.kind == Kind::Json {
            return Ok(());
        }
        match event.kind() {
            "suite" => {
                if let (Some(id), Some(path)) = (event.u64_at(&["suite", "id"]), event.str_at(&["suite", "path"])) {
                    self.suites.insert(id, path.to_owned());
                }
            }
            "testStart" => self.test_start(event, out)?,
            "print" => self.print(event, out)?,
            "error" => self.error(event, out)?,
            "testDone" => self.test_done(event)?,
            crate::event::SHELL => {
                // The tool (or the runner) wrote the line itself, around the
                // reporter's output; it prints exactly as written.
                writeln!(out, "{}", event.str_at(&["message"]).unwrap_or(""))?;
            }
            _ => {}
        }
        Ok(())
    }

    fn test_start(&mut self, event: &Json, out: &mut dyn Write) -> io::Result<()> {
        let Some(id) = event.u64_at(&["test", "id"]) else {
            return Ok(());
        };
        let name = event.str_at(&["test", "name"]).unwrap_or("").to_owned();
        let groups = matches!(event.get("test").and_then(|test| test.get("groupIDs")), Some(Json::Array(ids)) if !ids.is_empty());
        let individual = name.rsplit(' ').next().unwrap_or("");
        let test = Test {
            load: !groups && name.starts_with("loading "),
            synthetic: individual == "(setUpAll)" || individual == "(tearDownAll)",
            skip: event.get("test").and_then(|test| test.get("metadata")).and_then(|meta| meta.get("skip")).and_then(Json::as_bool).unwrap_or(false),
            suite: event.u64_at(&["test", "suiteID"]).unwrap_or(u64::MAX),
            name,
            done: false,
            skipped: false,
            hidden: false,
            errors: Vec::new(),
            messages: Vec::new(),
            counted: false,
            result: "",
        };
        // The engine announces a starting test only when it is the one
        // running (`active.first`); a test starting beside another still
        // running prints nothing until it is that one.
        let solo = test.load || self.active.is_empty();
        if matches!(self.kind, Kind::Expanded) && solo {
            let description = self.description(&test);
            self.progress(out, &description, None, None)?;
        }
        if !test.load {
            self.active.push(id);
        }
        self.tests.insert(id, test);
        Ok(())
    }

    fn print(&mut self, event: &Json, out: &mut dyn Write) -> io::Result<()> {
        let Some(test) = event.u64_at(&["testID"]).and_then(|id| self.tests.get(&id)).cloned() else {
            return Ok(());
        };
        let message = event.str_at(&["message"]).unwrap_or("");
        let skip_type = event.str_at(&["messageType"]) == Some("skip");
        if matches!(self.kind, Kind::FailuresOnly) && test.skip {
            return Ok(());
        }
        let description = self.description(&test);
        self.progress(out, &description, None, None)?;
        if self.kind == Kind::Expanded && skip_type {
            writeln!(out, "  {}{message}{}", self.paint(YELLOW), self.paint(NO_COLOR))
        } else {
            writeln!(out, "{message}")
        }
    }

    fn error(&mut self, event: &Json, out: &mut dyn Write) -> io::Result<()> {
        let Some(id) = event.u64_at(&["testID"]) else {
            return Ok(());
        };
        let Some(test) = self.tests.get(&id).cloned() else {
            return Ok(());
        };
        // An error event is what a real failure or error always carries
        // before its `testDone`; its absence there marks the engine's
        // synthesized completion of a test whose suite was torn down.
        self.errored.insert(id);
        let error = event.str_at(&["error"]).unwrap_or("");
        let stack = event.str_at(&["stackTrace"]).unwrap_or("");
        if !test.load && !self.opts.chain_stack_traces {
            self.chain_notice = true;
        }
        // The runner's state change precedes the error report, and an
        // error after completion moves a passed test to failed.
        if !test.counted {
            if test.done && !test.hidden && !test.skipped {
                self.passed -= 1;
            }
            self.failed += 1;
            let path = self.suites.get(&test.suite).cloned().unwrap_or_default();
            self.failures.push((path, test.name.clone()));
            if let Some(entry) = self.tests.get_mut(&id) {
                entry.counted = true;
            }
        }
        // An error after completion changes the completed state (to `error`,
        // or to `failure` for a test failure on a passed test); the expanded
        // reporter answers a state change with the oldest active test.
        if test.done && !test.load {
            let failure = event.get("isFailure").and_then(Json::as_bool) == Some(true);
            let next = if failure && test.result != "error" { "failure" } else { "error" };
            if next != test.result {
                if let Some(entry) = self.tests.get_mut(&id) {
                    entry.result = next;
                }
                if self.kind == Kind::Expanded {
                    if let Some(first) = self.active.first().and_then(|first| self.tests.get(first)).cloned() {
                        let description = self.description(&first);
                        self.progress(out, &description, None, None)?;
                    }
                }
            }
        }
        let suffix = format!(" {}{}[E]{}", self.paint(BOLD), self.paint(RED), self.paint(NO_COLOR));
        let description = self.description(&test);
        self.progress(out, &description, None, Some(&suffix))?;
        if !test.load {
            writeln!(out, "{}", indent(error))?;
            return writeln!(out, "{}", indent(stack));
        }
        // A load failure renders as the runner's `LoadException`, whose
        // header is red under color; its trace is printed only when the
        // underlying error is a thrown object rather than a message (a
        // compile error, a missing file), which shows as a trace with a
        // frame outside the runner's own code.
        let rendered = match error.split_once(':') {
            Some((head, rest)) if self.opts.color && head.starts_with("Failed to load \"") => {
                format!("{RED}{head}:{NO_COLOR}{rest}")
            }
            _ => error.to_owned(),
        };
        writeln!(out, "{}", indent(&rendered))?;
        if user_frames(stack) {
            writeln!(out, "{}", indent(stack))?;
        }
        Ok(())
    }

    fn test_done(&mut self, event: &Json) -> io::Result<()> {
        let Some(id) = event.u64_at(&["testID"]) else {
            return Ok(());
        };
        let hidden = event.get("hidden").and_then(Json::as_bool).unwrap_or(false);
        let skipped = event.get("skipped").and_then(Json::as_bool).unwrap_or(false);
        let result = event.str_at(&["result"]).unwrap_or("").to_owned();
        let Some(test) = self.tests.get_mut(&id) else {
            return Ok(());
        };
        // A non-success outcome no error event announced is the engine's
        // synthesized completion of a test whose suite was torn down (a
        // suite that called `exit()`): its outcome is unknown, so it counts
        // nowhere and prints as not having completed.
        if result != "success" && !skipped && !self.errored.contains(&id) {
            test.result = "error";
            return Ok(());
        }
        test.done = true;
        test.hidden = hidden;
        test.skipped = skipped;
        test.result = match result.as_str() {
            "success" => "success",
            "failure" => "failure",
            _ => "error",
        };
        self.active.retain(|&active| active != id);
        if hidden {
            return Ok(());
        }
        self.live += 1;
        if skipped {
            self.skipped += 1;
        } else if result == "success" && !test.counted {
            self.passed += 1;
        } else if !test.counted {
            let entry = (self.suites.get(&test.suite).cloned().unwrap_or_default(), test.name.clone());
            self.failed += 1;
            self.failures.push(entry);
        }
        Ok(())
    }

    /// The closing lines once every suite rendered.
    ///
    /// # Errors
    ///
    /// Write failures on `out`.
    pub fn done(&mut self, out: &mut dyn Write) -> io::Result<()> {
        if matches!(self.kind, Kind::Silent | Kind::Json) {
            return Ok(());
        }
        // A test the run ended on without completing (`exit()` mid-test): the
        // runner prints it once more with a marker, and the summary lists it.
        let unfinished: Vec<String> = self
            .active
            .iter()
            .filter_map(|id| self.tests.get(id).map(|test| self.description(test)))
            .collect();
        let incomplete: Vec<(String, String)> = self
            .active
            .iter()
            .filter_map(|id| self.tests.get(id))
            .map(|test| (self.suites.get(&test.suite).cloned().unwrap_or_default(), test.name.clone()))
            .collect();
        if self.live == 0 && unfinished.is_empty() {
            writeln!(out, "No tests ran.")?;
        } else if self.failed > 0 || !unfinished.is_empty() {
            let suffix = format!(" - did not complete {}{}[E]{}", self.paint(BOLD), self.paint(RED), self.paint(NO_COLOR));
            for description in &unfinished {
                self.progress(out, description, None, Some(&suffix))?;
            }
            self.progress(out, "Some tests failed.", Some(RED), None)?;
            if self.kind == Kind::Expanded {
                self.summary(out, incomplete)?;
            }
        } else if self.passed == 0 {
            let message = if self.kind == Kind::Expanded {
                "All tests skipped.".to_owned()
            } else {
                format!("{}All tests skipped.{}", self.paint(YELLOW), self.paint(NO_COLOR))
            };
            self.progress(out, &message, None, None)?;
        } else if self.kind == Kind::Expanded || self.skipped == 0 {
            self.progress(out, "All tests passed!", None, None)?;
        } else {
            let noun = if self.skipped == 1 { "test" } else { "tests" };
            let message = format!("{}{} skipped {noun}.{}", self.paint(YELLOW), self.skipped, self.paint(NO_COLOR));
            self.progress(out, &message, None, None)?;
            self.progress(out, "All other tests passed!", None, None)?;
        }
        if self.chain_notice {
            writeln!(out)?;
            writeln!(out, "Consider enabling the flag chain-stack-traces to receive more detailed exceptions.")?;
            writeln!(out, "For example, 'dart test --chain-stack-traces'.")?;
        }
        Ok(())
    }

    /// The expanded reporter's `Failing tests:` list: sorted by path then
    /// name, at most five entries; `incomplete` are the tests the run ended
    /// on without completing, each annotated.
    fn summary(&mut self, out: &mut dyn Write, incomplete: Vec<(String, String)>) -> io::Result<()> {
        let mut entries: Vec<(String, String, bool)> = self
            .failures
            .iter()
            .map(|(path, name)| (path.clone(), name.clone(), false))
            .chain(incomplete.into_iter().map(|(path, name)| (path, name, true)))
            .collect();
        if entries.is_empty() {
            return Ok(());
        }
        entries.sort_unstable_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
        writeln!(out)?;
        writeln!(out, "{}Failing tests:{}", self.paint(RED), self.paint(NO_COLOR))?;
        let shown = if entries.len() > 5 { 4 } else { entries.len() };
        for (path, name, incomplete) in &entries[..shown] {
            let suffix = if *incomplete { " (did not complete)" } else { "" };
            writeln!(out, "  {path}: {name}{suffix}")?;
        }
        if entries.len() > shown {
            writeln!(out, "  ... and {} more", entries.len() - shown)?;
        }
        Ok(())
    }
}

/// A trace with a frame outside the runner's own code and the SDK.
fn user_frames(stack: &str) -> bool {
    stack.lines().any(|line| {
        let frame = line.trim();
        !frame.is_empty()
            && !frame.starts_with("package:test_core/")
            && !frame.starts_with("package:test_api/")
            && !frame.starts_with("package:async/")
            && !frame.starts_with("package:stream_channel/")
            && !frame.starts_with("dart:")
            && !frame.starts_with("=====")
            && !frame.starts_with("<asynchronous")
    })
}

/// `test_core`'s `truncate` (`utils.dart`), on UTF-16 lengths as Dart
/// strings measure them.
fn truncate(text: &str, max: usize) -> String {
    let len = |s: &str| s.encode_utf16().count();
    if len(text) <= max {
        return text.to_owned();
    }
    let words: Vec<&str> = text.split(' ').collect();
    if words.len() > 1 {
        let mut i = words.len();
        let mut length = len(words[0]) + 4;
        loop {
            i -= 1;
            length += 1 + len(words[i]);
            if !(length <= max && i > 0) {
                break;
            }
        }
        if length > max || i == 0 {
            i += 1;
        }
        if i + 4 < words.len() {
            let mut out = format!("{} ...", words[0]);
            for word in &words[i..] {
                out.push(' ');
                out.push_str(word);
            }
            return out;
        }
    }
    // The trailing `max - 4` code units, from a word start when possible.
    let units: Vec<u16> = text.encode_utf16().collect();
    let tail = String::from_utf16_lossy(&units[units.len() - max.saturating_sub(4).min(units.len())..]);
    let tail = match tail.find(' ') {
        Some(space) if space > 0 => tail[space..].to_owned(),
        _ => tail,
    };
    format!("...{tail}")
}

/// A compact-reporter test: what its state machine needs.
#[derive(Debug, Clone)]
struct Live {
    name: String,
    suite: u64,
    load: bool,
    skip: bool,
    /// `Status.complete` reached, and the result then.
    done: bool,
    result: &'static str,
    hidden: bool,
}

/// The `compact` reporter (`compact.dart`): one progress line rewritten in
/// place with `\r`, driven by the engine's test starts, state changes,
/// errors, messages and completions.
pub struct Compact {
    opts: Options,
    started: Instant,
    suites: rustc_hash::FxHashMap<u64, String>,
    tests: rustc_hash::FxHashMap<u64, Live>,
    /// The engine's active user tests and active suite loads, in start order.
    active: Vec<u64>,
    loads: Vec<u64>,
    passed: usize,
    skipped: usize,
    failed: usize,
    /// User tests seen (the engine's `liveTests`).
    live: usize,
    /// The last progress line's counts, message, suffix and truncation.
    last: Option<(usize, usize, usize, String, Option<String>, bool)>,
    /// Elapsed whole seconds when the last progress line printed.
    last_elapsed: u64,
    printed_newline: bool,
    chain_notice: bool,
}

impl Compact {
    #[must_use]
    pub fn new(opts: Options) -> Self {
        Self {
            opts,
            started: Instant::now(),
            suites: rustc_hash::FxHashMap::default(),
            tests: rustc_hash::FxHashMap::default(),
            active: Vec::new(),
            loads: Vec::new(),
            passed: 0,
            skipped: 0,
            failed: 0,
            live: 0,
            last: None,
            last_elapsed: 0,
            printed_newline: true,
            chain_notice: false,
        }
    }

    fn paint(&self, code: &'static str) -> &'static str {
        if self.opts.color { code } else { "" }
    }

    fn description(&self, test: &Live) -> String {
        let mut name = test.name.clone();
        if self.opts.print_path && !test.load {
            if let Some(path) = self.suites.get(&test.suite) {
                name = format!("{path}: {name}");
            }
        }
        if test.load {
            name = format!("{}{}{name}{}", self.paint(BOLD), self.paint(GRAY), self.paint(NO_COLOR));
        }
        name
    }

    fn describe(&self, id: u64) -> String {
        self.tests.get(&id).map(|test| self.description(test)).unwrap_or_default()
    }

    /// `_progressLine`: unchanged counts, message, suffix and truncation
    /// print nothing, unless the line is still the terminal's last and the
    /// elapsed second changed.
    fn progress(&mut self, out: &mut dyn Write, message: &str, color: &str, truncate_line: bool, suffix: Option<&str>) -> io::Result<()> {
        let elapsed = self.started.elapsed().as_secs();
        if let Some(last) = &self.last {
            if last.0 == self.passed
                && last.1 == self.skipped
                && last.2 == self.failed
                && last.3 == message
                && (suffix.is_none() || last.4.as_deref() == suffix)
                && (truncate_line || !last.5)
                && (self.printed_newline || elapsed == self.last_elapsed)
            {
                return Ok(());
            }
        }
        self.last = Some((self.passed, self.skipped, self.failed, message.to_owned(), suffix.map(str::to_owned), truncate_line));
        self.last_elapsed = elapsed;
        let mut line = format!("\r{:02}:{:02} {}+{}{}", elapsed / 60, elapsed % 60, self.paint(GREEN), self.passed, self.paint(NO_COLOR));
        if self.skipped > 0 {
            line.push_str(&format!("{} ~{}{}", self.paint(YELLOW), self.skipped, self.paint(NO_COLOR)));
        }
        if self.failed > 0 {
            line.push_str(&format!("{} -{}{}", self.paint(RED), self.failed, self.paint(NO_COLOR)));
        }
        line.push_str(": ");
        line.push_str(color);
        let mut message = message.to_owned();
        if let Some(suffix) = suffix {
            message.push_str(suffix);
        }
        let width = |text: &str| strip_colors(text).encode_utf16().count();
        if truncate_line {
            message = truncate(&message, self.opts.line_length.saturating_sub(width(&line)));
        }
        line.push_str(&message);
        line.push_str(self.paint(NO_COLOR));
        let pad = self.opts.line_length.saturating_sub(width(&line));
        write!(out, "{line}{}", " ".repeat(pad))?;
        self.printed_newline = false;
        Ok(())
    }

    fn newline(&mut self, out: &mut dyn Write) -> io::Result<()> {
        if !self.printed_newline {
            writeln!(out)?;
        }
        self.printed_newline = true;
        Ok(())
    }

    /// Render one suite's events.
    ///
    /// # Errors
    ///
    /// Write failures on `out`.
    pub fn block(&mut self, events: &[Json], out: &mut dyn Write) -> io::Result<()> {
        for event in events {
            match event.kind() {
                "suite" => {
                    if let (Some(id), Some(path)) = (event.u64_at(&["suite", "id"]), event.str_at(&["suite", "path"])) {
                        self.suites.insert(id, path.to_owned());
                    }
                }
                "testStart" => self.start(event, out)?,
                "print" => self.message(event, out)?,
                "error" => self.error(event, out)?,
                "testDone" => self.complete(event, out)?,
                crate::event::SHELL => {
                    // The line follows the in-place progress bytes as the
                    // tool wrote it: no progress of its own, no blank line.
                    writeln!(out, "{}", event.str_at(&["message"]).unwrap_or(""))?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn start(&mut self, event: &Json, out: &mut dyn Write) -> io::Result<()> {
        let Some(id) = event.u64_at(&["test", "id"]) else { return Ok(()) };
        let name = event.str_at(&["test", "name"]).unwrap_or("").to_owned();
        let groups = matches!(event.get("test").and_then(|test| test.get("groupIDs")), Some(Json::Array(ids)) if !ids.is_empty());
        let test = Live {
            load: !groups && name.starts_with("loading "),
            skip: event.get("test").and_then(|test| test.get("metadata")).and_then(|meta| meta.get("skip")).and_then(Json::as_bool).unwrap_or(false),
            suite: event.u64_at(&["test", "suiteID"]).unwrap_or(u64::MAX),
            name,
            done: false,
            result: "success",
            hidden: false,
        };
        if test.load { self.loads.push(id) } else { self.active.push(id) }
        let load = test.load;
        self.tests.insert(id, test);
        let first = if load { self.active.is_empty() && self.loads.len() == 1 } else { self.active.len() == 1 };
        if first {
            let description = self.describe(id);
            self.progress(out, &description, "", true, None)?;
        }
        Ok(())
    }

    fn message(&mut self, event: &Json, out: &mut dyn Write) -> io::Result<()> {
        let Some(id) = event.u64_at(&["testID"]) else { return Ok(()) };
        let Some(test) = self.tests.get(&id) else { return Ok(()) };
        if test.skip {
            return Ok(());
        }
        let description = self.description(test);
        self.progress(out, &description, "", false, None)?;
        self.newline(out)?;
        writeln!(out, "{}", event.str_at(&["message"]).unwrap_or(""))
    }

    /// Move a test into the engine's failed set (once): the invoker's
    /// `setState(complete, error|failure)` that precedes every error report.
    /// A passed or skipped test leaves its set.
    fn fail(&mut self, id: u64) {
        let Some(test) = self.tests.get_mut(&id) else { return };
        let was = if test.done { test.result } else { "running" };
        if was == "failed" {
            return;
        }
        test.result = "failed";
        test.done = true;
        match was {
            "success" if !test.hidden => self.passed -= 1,
            "skipped" if !test.hidden => self.skipped -= 1,
            _ => {}
        }
        self.failed += 1;
        self.active.retain(|&active| active != id);
        self.loads.retain(|&load| load != id);
    }

    fn error(&mut self, event: &Json, out: &mut dyn Write) -> io::Result<()> {
        let Some(id) = event.u64_at(&["testID"]) else { return Ok(()) };
        let Some(test) = self.tests.get(&id).cloned() else { return Ok(()) };
        if !test.load && !self.opts.chain_stack_traces {
            self.chain_notice = true;
        }
        // `_onStateChange` ignores a failing result; `_onError` prints.
        self.fail(id);
        let suffix = format!(" {}{}[E]{}", self.paint(BOLD), self.paint(RED), self.paint(NO_COLOR));
        let description = self.description(&test);
        self.progress(out, &description, "", false, Some(&suffix))?;
        self.newline(out)?;
        let error = event.str_at(&["error"]).unwrap_or("");
        let stack = event.str_at(&["stackTrace"]).unwrap_or("");
        if !test.load {
            writeln!(out, "{}", indent(error))?;
            return writeln!(out, "{}", indent(stack));
        }
        let rendered = match error.split_once(':') {
            Some((head, rest)) if self.opts.color && head.starts_with("Failed to load \"") => format!("{RED}{head}:{NO_COLOR}{rest}"),
            _ => error.to_owned(),
        };
        writeln!(out, "{}", indent(&rendered))?;
        if user_frames(stack) {
            writeln!(out, "{}", indent(stack))?;
        }
        Ok(())
    }

    fn complete(&mut self, event: &Json, out: &mut dyn Write) -> io::Result<()> {
        let Some(id) = event.u64_at(&["testID"]) else { return Ok(()) };
        let hidden = event.get("hidden").and_then(Json::as_bool).unwrap_or(false);
        let skipped = event.get("skipped").and_then(Json::as_bool).unwrap_or(false);
        let result = event.str_at(&["result"]).unwrap_or("");
        let Some(test) = self.tests.get_mut(&id) else { return Ok(()) };
        test.hidden = hidden;
        if test.result != "failed" {
            // A non-success outcome no error event announced is the engine's
            // synthesized completion of a torn-down suite: the test counts
            // nowhere and prints as not having completed.
            if result != "success" && !skipped {
                return Ok(());
            }
            // Completed without an error: `_onStateChange` shows the oldest
            // active test, else this one.
            test.done = true;
            test.result = if skipped { "skipped" } else { "success" };
            if !hidden {
                if skipped { self.skipped += 1 } else { self.passed += 1 }
            }
            self.active.retain(|&active| active != id);
            self.loads.retain(|&load| load != id);
            let next = self.active.first().copied().unwrap_or(id);
            let description = self.describe(next);
            self.progress(out, &description, "", true, None)?;
        }
        let test = self.tests[&id].clone();
        if !hidden || test.result == "failed" {
            self.live += 1;
        }
        if test.result == "failed" {
            // `onComplete`: the rerun hint, on a line of its own.
            let path = self.suites.get(&test.suite).cloned().unwrap_or_default();
            let quoted = format!("'{}'", test.name.replace('\'', "'\\''"));
            writeln!(out)?;
            writeln!(
                out,
                "{}{}To run this test again:{} {} test {path} -p vm --plain-name {quoted}",
                self.paint(BOLD),
                self.paint(CYAN),
                self.paint(NO_COLOR),
                self.opts.executable
            )?;
        }
        Ok(())
    }

    /// The closing lines once every suite rendered.
    ///
    /// # Errors
    ///
    /// Write failures on `out`.
    pub fn done(&mut self, out: &mut dyn Write) -> io::Result<()> {
        // Tests the run ended on without completing: one line each, uncut,
        // then a blank line, as the runner prints them.
        let unfinished: Vec<String> = self.active.iter().map(|&id| self.describe(id)).collect();
        if self.live == 0 && unfinished.is_empty() {
            if !self.printed_newline {
                write!(out, "\r")?;
            }
            let message = "No tests ran.";
            write!(out, "{message}")?;
            if !self.printed_newline {
                write!(out, "{}", " ".repeat(self.opts.line_length.saturating_sub(message.len())))?;
            }
            writeln!(out)?;
        } else if self.failed > 0 || !unfinished.is_empty() {
            let suffix = format!(" - did not complete {}{}[E]{}", self.paint(BOLD), self.paint(RED), self.paint(NO_COLOR));
            for description in &unfinished {
                self.progress(out, description, "", false, Some(&suffix))?;
                writeln!(out)?;
            }
            let red = self.paint(RED);
            self.progress(out, "Some tests failed.", red, true, None)?;
            writeln!(out)?;
        } else if self.passed == 0 {
            let message = format!("{}All tests skipped.{}", self.paint(YELLOW), self.paint(NO_COLOR));
            self.progress(out, &message, "", true, None)?;
            writeln!(out)?;
        } else if self.skipped == 0 {
            self.progress(out, "All tests passed!", "", true, None)?;
            writeln!(out)?;
        } else {
            let noun = if self.skipped == 1 { "test" } else { "tests" };
            let message = format!("{}{} skipped {noun}.{}", self.paint(YELLOW), self.skipped, self.paint(NO_COLOR));
            self.progress(out, &message, "", true, None)?;
            writeln!(out)?;
            self.progress(out, "All other tests passed!", "", true, None)?;
            writeln!(out)?;
        }
        if self.chain_notice {
            writeln!(out)?;
            writeln!(out, "Consider enabling the flag chain-stack-traces to receive more detailed exceptions.")?;
            writeln!(out, "For example, 'dart test --chain-stack-traces'.")?;
        }
        Ok(())
    }
}

/// The `github` reporter: GitHub Actions log groups.
pub struct Github {
    /// The engine `flutter test` pins (`test_core` 0.6.18): skipped tests
    /// print in place in a `⏭️ Skipped tests` group of their own, and a
    /// test completing after its group switches closes it.
    pub legacy: bool,
    /// The `::group::` currently open.
    open: Open,
    print_path: bool,
    suites: rustc_hash::FxHashMap<u64, String>,
    tests: rustc_hash::FxHashMap<u64, Test>,
    skipped_tests: Vec<u64>,
    passed: usize,
    failed: usize,
    skipped: usize,
}

impl Github {
    #[must_use]
    pub fn new(print_path: bool) -> Self {
        Self {
            legacy: false,
            open: Open::None,
            print_path,
            suites: rustc_hash::FxHashMap::default(),
            tests: rustc_hash::FxHashMap::default(),
            skipped_tests: Vec::new(),
            passed: 0,
            failed: 0,
            skipped: 0,
        }
    }

    /// The spacer the runner writes ahead of any result.
    ///
    /// # Errors
    ///
    /// Write failures on `out`.
    pub fn start(&mut self, out: &mut dyn Write) -> io::Result<()> {
        writeln!(out)
    }

    fn name(&self, test: &Test) -> String {
        match self.suites.get(&test.suite) {
            Some(path) if self.print_path && !test.load => format!("{path}: {}", test.name),
            _ => test.name.clone(),
        }
    }

    /// Close whatever group is open.
    fn end_passing(&mut self, out: &mut dyn Write) -> io::Result<()> {
        if self.open != Open::None {
            writeln!(out, "::endgroup::")?;
            self.open = Open::None;
        }
        Ok(())
    }

    /// Open `group` (`Passing` or `Skipped`), closing another one first.
    fn enter(&mut self, group: Open, title: &str, out: &mut dyn Write) -> io::Result<()> {
        if self.open != group {
            self.end_passing(out)?;
            writeln!(out, "::group::{title}")?;
            self.open = group;
        }
        Ok(())
    }

    /// Render one suite's events.
    ///
    /// # Errors
    ///
    /// Write failures on `out`.
    pub fn block(&mut self, events: &[Json], out: &mut dyn Write) -> io::Result<()> {
        for event in events {
            match event.kind() {
                "suite" => {
                    if let (Some(id), Some(path)) = (event.u64_at(&["suite", "id"]), event.str_at(&["suite", "path"])) {
                        self.suites.insert(id, path.to_owned());
                    }
                }
                crate::event::SHELL => {
                    // The tool writes the line around the reporter's groups;
                    // it lands inside whichever group is open.
                    writeln!(out, "{}", event.str_at(&["message"]).unwrap_or(""))?;
                }
                "testStart" => {
                    let Some(id) = event.u64_at(&["test", "id"]) else { continue };
                    let name = event.str_at(&["test", "name"]).unwrap_or("").to_owned();
                    let groups = matches!(event.get("test").and_then(|test| test.get("groupIDs")), Some(Json::Array(ids)) if !ids.is_empty());
                    let individual = name.rsplit(' ').next().unwrap_or("");
                    self.tests.insert(id, Test {
                        load: !groups && name.starts_with("loading "),
                        synthetic: individual == "(setUpAll)" || individual == "(tearDownAll)",
                        skip: false,
                        suite: event.u64_at(&["test", "suiteID"]).unwrap_or(u64::MAX),
                        name,
                        done: false,
                        skipped: false,
                        hidden: false,
                        errors: Vec::new(),
                        messages: Vec::new(),
                        counted: false,
                        result: "",
                    });
                }
                "print" => {
                    let Some(id) = event.u64_at(&["testID"]) else { continue };
                    let message = event.str_at(&["message"]).unwrap_or("").to_owned();
                    let kind = event.str_at(&["messageType"]).unwrap_or("").to_owned();
                    let Some(test) = self.tests.get_mut(&id) else { continue };
                    // The legacy engine prints a skipped test's late
                    // output in place too.
                    if test.done && (!test.skipped || self.legacy) {
                        self.end_passing(out)?;
                        writeln!(out, "{message}")?;
                    } else {
                        test.messages.push((kind, message));
                    }
                }
                "error" => {
                    let Some(id) = event.u64_at(&["testID"]) else { continue };
                    let error = event.str_at(&["error"]).unwrap_or("").to_owned();
                    let stack = event.str_at(&["stackTrace"]).unwrap_or("").to_owned();
                    let Some(test) = self.tests.get(&id).cloned() else { continue };
                    if test.done {
                        // The completed test joins the engine's failed set;
                        // a passed one leaves the passed set (a skipped one
                        // stays counted as skipped too).
                        let failed_already = test.result != "success" && !test.skipped;
                        if !test.hidden && !test.counted && !failed_already {
                            if !test.skipped {
                                self.passed -= 1;
                            }
                            self.failed += 1;
                            if let Some(entry) = self.tests.get_mut(&id) {
                                entry.counted = true;
                            }
                        }
                        let name = self.name(&test);
                        self.end_passing(out)?;
                        writeln!(out, "::group::{}", format!("❌ {name} (failed after test completion)").replace('\n', " "))?;
                        writeln!(out, "{error}")?;
                        writeln!(out, "{}", stack.trim_end())?;
                        writeln!(out, "::endgroup::")?;
                    } else if let Some(test) = self.tests.get_mut(&id) {
                        test.errors.push((error, stack));
                    }
                }
                "testDone" => {
                    let Some(id) = event.u64_at(&["testID"]) else { continue };
                    let hidden = event.get("hidden").and_then(Json::as_bool).unwrap_or(false);
                    let skipped = event.get("skipped").and_then(Json::as_bool).unwrap_or(false);
                    let result = event.str_at(&["result"]).unwrap_or("").to_owned();
                    let Some(test) = self.tests.get_mut(&id) else { continue };
                    test.done = true;
                    test.skipped = skipped;
                    test.hidden = hidden;
                    test.result = if result == "success" && test.errors.is_empty() { "success" } else { "error" };
                    let test = test.clone();
                    if !hidden {
                        if skipped {
                            self.skipped += 1;
                        } else if test.result == "success" {
                            self.passed += 1;
                        } else {
                            self.failed += 1;
                        }
                    }
                    let synthetic = test.load || test.synthetic;
                    if synthetic && test.errors.is_empty() && test.messages.is_empty() {
                        continue;
                    }
                    if skipped && test.errors.is_empty() {
                        if self.legacy {
                            let name = self.name(&test);
                            self.enter(Open::Skipped, "⏭️ Skipped tests", out)?;
                            writeln!(out, "⏭️ {name} (skipped)")?;
                            for (_, message) in &test.messages {
                                writeln!(out, "{message}")?;
                            }
                        } else {
                            self.skipped_tests.push(id);
                        }
                        continue;
                    }
                    let name = self.name(&test);
                    if test.errors.is_empty() {
                        if self.legacy && !test.messages.is_empty() {
                            // The legacy engine groups a passing test that
                            // printed on its own.
                            self.end_passing(out)?;
                            writeln!(out, "::group::✅ {name}")?;
                            for (_, message) in &test.messages {
                                writeln!(out, "{message}")?;
                            }
                            writeln!(out, "::endgroup::")?;
                            continue;
                        }
                        self.enter(Open::Passing, "✅ Passing tests", out)?;
                        writeln!(out, "✅ {name}")?;
                        for (_, message) in &test.messages {
                            writeln!(out, "{message}")?;
                        }
                    } else {
                        self.end_passing(out)?;
                        writeln!(out, "::group::{}", format!("❌ {name} (failed)").replace('\n', " "))?;
                        for (_, message) in &test.messages {
                            writeln!(out, "{message}")?;
                        }
                        for (error, stack) in &test.errors {
                            writeln!(out, "{error}")?;
                            writeln!(out, "{}", stack.trim_end())?;
                        }
                        writeln!(out, "::endgroup::")?;
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// The closing groups and the summary line.
    ///
    /// # Errors
    ///
    /// Write failures on `out`.
    pub fn done(&mut self, out: &mut dyn Write) -> io::Result<()> {
        self.end_passing(out)?;
        if !self.skipped_tests.is_empty() {
            writeln!(out, "::group::⏭️ Skipped tests")?;
            for id in std::mem::take(&mut self.skipped_tests) {
                let test = self.tests[&id].clone();
                writeln!(out, "⏭️ {} (skipped)", self.name(&test))?;
                for (_, message) in &test.messages {
                    writeln!(out, "{message}")?;
                }
            }
            writeln!(out, "::endgroup::")?;
        }
        writeln!(out)?;
        let noun = if self.passed == 1 { "test" } else { "tests" };
        let mut message = format!("{} {noun} passed", self.passed);
        if self.failed > 0 {
            message.push_str(&format!(", {} failed", self.failed));
        }
        if self.skipped > 0 {
            message.push_str(&format!(", {} skipped", self.skipped));
        }
        message.push('.');
        if self.failed > 0 {
            writeln!(out, "::error::{message}")
        } else {
            writeln!(out, "🎉 {message}")
        }
    }
}

/// The GitHub reporter's open log group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Open {
    None,
    Passing,
    Skipped,
}

/// Renumber the ids of one suite's events into a merged run's id space
/// (`next_id` is the merged run's next unused id) and move their `time`
/// stamps by `base` milliseconds.
pub fn rebase(events: &mut [Json], next_id: &mut u64, base: u64) {
    let mut ids: rustc_hash::FxHashMap<u64, u64> = rustc_hash::FxHashMap::default();
    for event in events {
        crate::event::remap(event, &mut |old| {
            *ids.entry(old).or_insert_with(|| {
                let id = *next_id;
                *next_id += 1;
                id
            })
        });
        if let Some(time) = event.u64_at(&["time"]) {
            event.set("time", (time + base).into());
        }
    }
}

/// The `json` reporter over several runs: one `start`, every suite's
/// events (already [`rebase`]d into one id space and one clock), then
/// `allSuites` and `done` — the shape one `dart test --reporter json` run
/// writes.
pub struct JsonMerge {
    started: Instant,
    suites: u64,
    /// The `allSuites` count, announced once the first suite has started
    /// loading (where a sequential run of the runner writes it; under
    /// concurrency the runner writes it after whichever suites happen to
    /// start first).
    total: u64,
    announced: bool,
    /// `flutter test --machine`: the tool writes a blank line and its
    /// `test.startedProcess` event once a suite's tester connected, before
    /// the suite's hidden load completes.
    pub machine: bool,
}

/// The tool's device event under `--machine`, as `EventPrinter` writes it.
const STARTED_PROCESS: &str = "\n[{\"event\":\"test.startedProcess\",\"params\":{\"vmServiceUri\":null}}]";

impl JsonMerge {
    /// Begin the stream with the runner's `start` event; `started` is the
    /// instant the merged run's clock counts from.
    ///
    /// # Errors
    ///
    /// Write failures on `out`.
    pub fn start(out: &mut dyn Write, runner_version: Option<&str>, started: Instant, total: u64) -> io::Result<Self> {
        let version = runner_version.map_or(Json::Null, |v| Json::String(v.to_owned()));
        let start = Json::Object(vec![
            ("protocolVersion".to_owned(), Json::String("0.1.1".to_owned())),
            ("runnerVersion".to_owned(), version),
            ("pid".to_owned(), u64::from(std::process::id()).into()),
            ("type".to_owned(), Json::String("start".to_owned())),
            ("time".to_owned(), 0u64.into()),
        ]);
        writeln!(out, "{}", start.to_line())?;
        Ok(Self { started, suites: 0, total, announced: false, machine: false })
    }

    fn now(&self) -> u64 {
        u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    /// Write one suite's events.
    ///
    /// # Errors
    ///
    /// Write failures on `out`.
    pub fn block(&mut self, events: &[Json], out: &mut dyn Write) -> io::Result<()> {
        self.suites += 1;
        let mut load: Option<u64> = None;
        for (index, event) in events.iter().enumerate() {
            if event.kind() == crate::event::SHELL {
                // The runner interleaved the raw line with its events; the
                // merged stream keeps it where it was.
                writeln!(out, "{}", event.str_at(&["message"]).unwrap_or(""))?;
                continue;
            }
            if self.machine && event.kind() == "testDone" && load.is_some() && event.u64_at(&["testID"]) == load {
                if !self.announced {
                    self.announce(event.u64_at(&["time"]), out)?;
                }
                writeln!(out, "{STARTED_PROCESS}")?;
                load = None;
            }
            writeln!(out, "{}", event.to_line())?;
            if index == 1 && event.kind() == "testStart" {
                load = event.u64_at(&["test", "id"]);
            }
            if !self.announced && index > 0 && event.kind() == "testStart" && !self.machine {
                self.announce(event.u64_at(&["time"]), out)?;
            }
        }
        if !self.announced {
            self.announce(None, out)?;
        }
        Ok(())
    }

    fn announce(&mut self, time: Option<u64>, out: &mut dyn Write) -> io::Result<()> {
        self.announced = true;
        let all = Json::Object(vec![
            ("count".to_owned(), self.total.into()),
            ("time".to_owned(), time.unwrap_or_else(|| self.now()).into()),
            ("type".to_owned(), Json::String("allSuites".to_owned())),
        ]);
        writeln!(out, "{}", all.to_line())
    }

    /// A runner-level event naming no suite.
    ///
    /// # Errors
    ///
    /// Write failures on `out`.
    pub fn loose(&mut self, event: &Json, out: &mut dyn Write) -> io::Result<()> {
        let mut event = event.clone();
        event.set("time", self.now().into());
        writeln!(out, "{}", event.to_line())
    }

    /// End the stream.
    ///
    /// # Errors
    ///
    /// Write failures on `out`.
    pub fn done(&mut self, success: bool, out: &mut dyn Write) -> io::Result<()> {
        if !self.announced {
            self.announce(None, out)?;
        }
        let done = Json::Object(vec![
            ("success".to_owned(), Json::Bool(success)),
            ("type".to_owned(), Json::String("done".to_owned())),
            ("time".to_owned(), self.now().into()),
        ]);
        writeln!(out, "{}", done.to_line())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::parse;

    fn events(lines: &[&str]) -> Vec<Json> {
        lines.iter().map(|line| parse(line).expect("event")).collect()
    }

    fn suite_a() -> Vec<Json> {
        events(&[
            r#"{"suite":{"id":0,"platform":"vm","path":"test/a_test.dart"},"type":"suite","time":0}"#,
            r#"{"test":{"id":1,"name":"loading test/a_test.dart","suiteID":0,"groupIDs":[],"metadata":{"skip":false,"skipReason":null}},"type":"testStart","time":1}"#,
            r#"{"testID":1,"result":"success","skipped":false,"hidden":true,"type":"testDone","time":5}"#,
            r#"{"group":{"id":2,"suiteID":0,"parentID":null,"name":"","testCount":3},"type":"group","time":5}"#,
            r#"{"group":{"id":3,"suiteID":0,"parentID":2,"name":"g","testCount":3},"type":"group","time":5}"#,
            r#"{"test":{"id":4,"name":"g passes","suiteID":0,"groupIDs":[2,3],"metadata":{"skip":false,"skipReason":null}},"type":"testStart","time":6}"#,
            r#"{"testID":4,"messageType":"print","message":"hello from a","type":"print","time":6}"#,
            r#"{"testID":4,"result":"success","skipped":false,"hidden":false,"type":"testDone","time":7}"#,
            r#"{"test":{"id":5,"name":"g fails","suiteID":0,"groupIDs":[2,3],"metadata":{"skip":false,"skipReason":null}},"type":"testStart","time":7}"#,
            r#"{"testID":5,"error":"Expected: <2>\n  Actual: <1>\n","stackTrace":"package:matcher        expect\ntest/a_test.dart 5:24  main.<fn>.<fn>\n","isFailure":true,"type":"error","time":8}"#,
            r#"{"testID":5,"result":"failure","skipped":false,"hidden":false,"type":"testDone","time":8}"#,
            r#"{"test":{"id":6,"name":"g skipped","suiteID":0,"groupIDs":[2,3],"metadata":{"skip":true,"skipReason":"not now"}},"type":"testStart","time":8}"#,
            r#"{"testID":6,"messageType":"skip","message":"Skip: not now","type":"print","time":8}"#,
            r#"{"testID":6,"result":"success","skipped":true,"hidden":false,"type":"testDone","time":8}"#,
        ])
    }

    fn render(kind: Kind, blocks: &[Vec<Json>]) -> String {
        let mut out = Vec::new();
        let mut renderer = Renderer::new(
            kind,
            Options { color: false, print_path: true, line_length: 200, executable: "dart".into(), chain_stack_traces: false },
        );
        for block in blocks {
            renderer.block(block, &mut out).expect("render");
        }
        renderer.done(&mut out).expect("done");
        String::from_utf8(out).expect("utf8")
    }

    /// The expanded reporter's lines for a passing, failing and skipped
    /// test, byte for byte as `dart test -r expanded -j 1` prints them.
    #[test]
    fn expanded_matches_the_runner() {
        assert_eq!(
            render(Kind::Expanded, &[suite_a()]),
            "00:00 +0: loading test/a_test.dart\n\
             00:00 +0: test/a_test.dart: g passes\n\
             hello from a\n\
             00:00 +1: test/a_test.dart: g fails\n\
             00:00 +1 -1: test/a_test.dart: g fails [E]\n  \
             Expected: <2>\n    \
             Actual: <1>\n  \n  \
             package:matcher        expect\n  \
             test/a_test.dart 5:24  main.<fn>.<fn>\n  \n\
             00:00 +1 -1: test/a_test.dart: g skipped\n  \
             Skip: not now\n\
             00:00 +1 ~1 -1: Some tests failed.\n\
             \n\
             Failing tests:\n  \
             test/a_test.dart: g fails\n\
             \n\
             Consider enabling the flag chain-stack-traces to receive more detailed exceptions.\n\
             For example, 'dart test --chain-stack-traces'.\n"
        );
    }

    /// A test the run ended on without completing (`exit()` mid-test) is
    /// printed once more with the runner's marker, and the summary lists it
    /// as such; a raw output line interleaved with a suite's events prints
    /// where the runner wrote it.
    #[test]
    fn unfinished_tests_and_raw_lines_match_the_runner() {
        let suite = events(&[
            r#"{"suite":{"id":0,"platform":"vm","path":"test/exit_test.dart"},"type":"suite","time":0}"#,
            r#"{"test":{"id":1,"name":"loading test/exit_test.dart","suiteID":0,"groupIDs":[],"metadata":{"skip":false,"skipReason":null}},"type":"testStart","time":0}"#,
            r#"{"testID":1,"result":"success","skipped":false,"hidden":true,"type":"testDone","time":1}"#,
            r#"{"test":{"id":2,"name":"exits","suiteID":0,"groupIDs":[],"metadata":{"skip":false,"skipReason":null}},"type":"testStart","time":1}"#,
            r#"{"type":"dartq.shell","message":"Shell: goodbye raw"}"#,
        ]);
        // No error event ever arrived (the run ended on the test), so the
        // runner prints no chaining notice.
        assert_eq!(
            render(Kind::Expanded, &[suite.clone()]),
            "00:00 +0: loading test/exit_test.dart\n\
             00:00 +0: test/exit_test.dart: exits\n\
             Shell: goodbye raw\n\
             00:00 +0: test/exit_test.dart: exits - did not complete [E]\n\
             00:00 +0: Some tests failed.\n\
             \n\
             Failing tests:\n  \
             test/exit_test.dart: exits (did not complete)\n"
        );
        // The failures-only and compact reporters print the same marker.
        assert!(render(Kind::FailuresOnly, &[suite.clone()]).contains("test/exit_test.dart: exits - did not complete [E]"));
        let mut out = Vec::new();
        let mut compact = Compact::new(Options { color: false, print_path: true, line_length: 200, executable: "dart".into(), chain_stack_traces: false });
        compact.block(&suite, &mut out).expect("block");
        compact.done(&mut out).expect("done");
        let text = String::from_utf8(out).expect("utf8");
        assert!(text.contains("Shell: goodbye raw"));
        assert!(text.contains("exits - did not complete [E]"));
    }

    #[test]
    fn failures_only_matches_the_runner() {
        assert_eq!(
            render(Kind::FailuresOnly, &[suite_a()]),
            "+0: test/a_test.dart: g passes\n\
             hello from a\n\
             +1 -1: test/a_test.dart: g fails [E]\n  \
             Expected: <2>\n    \
             Actual: <1>\n  \n  \
             package:matcher        expect\n  \
             test/a_test.dart 5:24  main.<fn>.<fn>\n  \n\
             +1 ~1 -1: Some tests failed.\n\
             \n\
             Consider enabling the flag chain-stack-traces to receive more detailed exceptions.\n\
             For example, 'dart test --chain-stack-traces'.\n"
        );
    }

    #[test]
    fn legacy_github_groups_skips_in_place() {
        let suite = events(&[
            r#"{"suite":{"id":0,"platform":"vm","path":"/p/test/a_test.dart"},"type":"suite","time":0}"#,
            r#"{"test":{"id":1,"name":"loading /p/test/a_test.dart","suiteID":0,"groupIDs":[],"metadata":{"skip":false,"skipReason":null}},"type":"testStart","time":0}"#,
            r#"{"testID":1,"result":"success","skipped":false,"hidden":true,"type":"testDone","time":1}"#,
            r#"{"test":{"id":2,"name":"one","suiteID":0,"groupIDs":[3],"metadata":{"skip":false,"skipReason":null}},"type":"testStart","time":1}"#,
            r#"{"testID":2,"result":"success","skipped":false,"hidden":false,"type":"testDone","time":2}"#,
            r#"{"test":{"id":4,"name":"two","suiteID":0,"groupIDs":[3],"metadata":{"skip":true,"skipReason":"later"}},"type":"testStart","time":2}"#,
            r#"{"testID":4,"messageType":"skip","message":"Skip: later","type":"print","time":2}"#,
            r#"{"testID":4,"result":"success","skipped":true,"hidden":false,"type":"testDone","time":2}"#,
            r#"{"test":{"id":5,"name":"three","suiteID":0,"groupIDs":[3],"metadata":{"skip":false,"skipReason":null}},"type":"testStart","time":3}"#,
            r#"{"testID":5,"result":"success","skipped":false,"hidden":false,"type":"testDone","time":4}"#,
        ]);
        let mut github = Github::new(false);
        github.legacy = true;
        let mut out = Vec::new();
        github.start(&mut out).expect("start");
        github.block(&suite, &mut out).expect("block");
        github.done(&mut out).expect("done");
        assert_eq!(
            String::from_utf8(out).expect("utf8"),
            "\n::group::✅ Passing tests\n✅ one\n::endgroup::\n::group::⏭️ Skipped tests\n⏭️ two (skipped)\nSkip: later\n::endgroup::\n::group::✅ Passing tests\n✅ three\n::endgroup::\n\n🎉 2 tests passed, 1 skipped.\n"
        );
    }

    #[test]
    fn github_matches_the_runner() {
        let mut out = Vec::new();
        let mut github = Github::new(true);
        github.start(&mut out).expect("start");
        github.block(&suite_a(), &mut out).expect("block");
        github.done(&mut out).expect("done");
        assert_eq!(
            String::from_utf8(out).expect("utf8"),
            "\n::group::✅ Passing tests\n\
             ✅ test/a_test.dart: g passes\n\
             hello from a\n\
             ::endgroup::\n\
             ::group::❌ test/a_test.dart: g fails (failed)\n\
             Expected: <2>\n  \
             Actual: <1>\n\n\
             package:matcher        expect\n\
             test/a_test.dart 5:24  main.<fn>.<fn>\n\
             ::endgroup::\n\
             ::group::⏭️ Skipped tests\n\
             ⏭️ test/a_test.dart: g skipped (skipped)\n\
             Skip: not now\n\
             ::endgroup::\n\
             \n\
             ::error::1 test passed, 1 failed, 1 skipped.\n"
        );
    }

    /// Two runs' blocks merge into one id space: no id repeats, every
    /// reference follows its object.
    #[test]
    fn json_merge_renumbers_ids() {
        let mut out = Vec::new();
        let mut merge = JsonMerge::start(&mut out, Some("1.0.0"), Instant::now(), 2).expect("start");
        let mut next = 0;
        for _ in 0..2 {
            let mut block = suite_a();
            rebase(&mut block, &mut next, 10);
            merge.block(&block, &mut out).expect("block");
        }
        merge.done(false, &mut out).expect("done");
        let text = String::from_utf8(out).expect("utf8");
        let lines: Vec<Json> = text.lines().map(|line| parse(line).expect("event")).collect();
        assert_eq!(lines.first().map(Json::kind), Some("start"));
        assert_eq!(lines.last().map(Json::kind), Some("done"));
        let suites: Vec<u64> = lines.iter().filter_map(|e| e.u64_at(&["suite", "id"])).collect();
        assert_eq!(suites, [0, 7]);
        let second_start = lines.iter().filter(|e| e.kind() == "testStart").nth(4).expect("start");
        assert_eq!(second_start.u64_at(&["test", "suiteID"]), Some(7));
        assert!(text.contains(r#"{"count":2,"#));
    }

    /// The compact reporter's lines for the same suite, as `dart test -r
    /// compact -j 1` prints them: progress rewritten in place with `\r`,
    /// padded to the terminal width, the failure uncut with its rerun hint.
    #[test]
    fn compact_matches_the_runner() {
        let mut out = Vec::new();
        let mut compact = Compact::new(Options {
            color: false,
            print_path: true,
            line_length: 200,
            executable: "dart".into(),
            chain_stack_traces: false,
        });
        compact.block(&suite_a(), &mut out).expect("block");
        compact.done(&mut out).expect("done");
        let text = String::from_utf8(out).expect("utf8");
        // Every in-place progress segment pads to the terminal width (the
        // `\r` rides in front of it, outside the segment).
        let first = text.split('\n').next().expect("line");
        let segments: Vec<&str> = first.split('\r').collect();
        assert!(segments.first().is_some_and(|piece| piece.is_empty()), "the run opens with \\r");
        assert!(segments.iter().skip(1).all(|segment| segment.len() + 1 == 200), "{segments:?}");
        // The reporter rewrites in place: each `\r` segment, and each
        // newline-terminated line, is one printed piece.
        let body = text.strip_prefix('\r').expect("opens with \\r");
        let body = body.strip_suffix('\n').expect("ends with a newline");
        let pieces: Vec<String> = body.split(['\r', '\n']).map(|piece| piece.trim_end().to_owned()).collect();
        assert_eq!(
            pieces,
            [
                "00:00 +0: loading test/a_test.dart",
                "00:00 +0: test/a_test.dart: g passes",
                "00:00 +0: test/a_test.dart: g passes",
                "hello from a",
                "",
                "00:00 +1: test/a_test.dart: g passes",
                "00:00 +1: test/a_test.dart: g fails",
                "00:00 +1 -1: test/a_test.dart: g fails [E]",
                "  Expected: <2>",
                "    Actual: <1>",
                "",
                "  package:matcher        expect",
                "  test/a_test.dart 5:24  main.<fn>.<fn>",
                "",
                "",
                "To run this test again: dart test test/a_test.dart -p vm --plain-name 'g fails'",
                "",
                "00:00 +1 -1: test/a_test.dart: g skipped",
                "00:00 +1 ~1 -1: test/a_test.dart: g skipped",
                "00:00 +1 ~1 -1: Some tests failed.",
                "",
                "Consider enabling the flag chain-stack-traces to receive more detailed exceptions.",
                "For example, 'dart test --chain-stack-traces'.",
            ]
        );
    }

    /// A name longer than the terminal is cut the runner's own `truncate`
    /// spells it: the trailing characters behind a `...` prefix.
    #[test]
    fn compact_truncates_to_the_terminal_width() {
        let suite = events(&[
            r#"{"suite":{"id":0,"platform":"vm","path":"test/a_test.dart"},"type":"suite","time":0}"#,
            r#"{"test":{"id":1,"name":"loading test/a_test.dart","suiteID":0,"groupIDs":[],"metadata":{"skip":false,"skipReason":null}},"type":"testStart","time":0}"#,
            r#"{"testID":1,"result":"success","skipped":false,"hidden":true,"type":"testDone","time":1}"#,
            &format!(
                r#"{{"test":{{"id":2,"name":"{}","suiteID":0,"groupIDs":[3],"metadata":{{"skip":false,"skipReason":null}}}},"type":"testStart","time":1}}"#,
                "a".repeat(40)
            ),
            r#"{"testID":2,"result":"success","skipped":false,"hidden":false,"type":"testDone","time":2}"#,
        ]);
        let mut out = Vec::new();
        let mut compact = Compact::new(Options {
            color: false,
            print_path: true,
            line_length: 40,
            executable: "dart".into(),
            chain_stack_traces: true,
        });
        compact.block(&suite, &mut out).expect("block");
        compact.done(&mut out).expect("done");
        let text = String::from_utf8(out).expect("utf8");
        let body = text.strip_prefix('\r').expect("opens with \\r");
        let body = body.strip_suffix('\n').expect("ends with a newline");
        let pieces: Vec<&str> = body.split(['\r', '\n']).collect();
        let truncated = format!("00:00 +0: ...{}", "a".repeat(25));
        assert_eq!(pieces[1].trim_end(), truncated, "the long name is cut");
        assert!(pieces.iter().all(|piece| piece.trim_end().len() <= 40), "{pieces:?}");
        assert!(text.contains("All tests passed!"));
    }

    /// A test that fails after it completed: the passed count gives the
    /// test back, the failure gets its group, and its late output prints
    /// outside every group, as the reporter does.
    #[test]
    fn github_reports_failures_after_completion() {
        let suite = events(&[
            r#"{"suite":{"id":0,"platform":"vm","path":"test/a_test.dart"},"type":"suite","time":0}"#,
            r#"{"test":{"id":1,"name":"t","suiteID":0,"groupIDs":[],"metadata":{"skip":false,"skipReason":null}},"type":"testStart","time":0}"#,
            r#"{"testID":1,"result":"success","skipped":false,"hidden":false,"type":"testDone","time":1}"#,
            r#"{"testID":1,"error":"boom","stackTrace":"frame","isFailure":true,"type":"error","time":2}"#,
            r#"{"testID":1,"messageType":"print","message":"late","type":"print","time":2}"#,
        ]);
        let mut out = Vec::new();
        let mut github = Github::new(false);
        github.start(&mut out).expect("start");
        github.block(&suite, &mut out).expect("block");
        github.done(&mut out).expect("done");
        assert_eq!(
            String::from_utf8(out).expect("utf8"),
            "\n::group::✅ Passing tests\n✅ t\n::endgroup::\n::group::❌ t (failed after test completion)\nboom\nframe\n::endgroup::\nlate\n\n::error::0 tests passed, 1 failed.\n"
        );
    }

    /// Under `--machine` the tool's `test.startedProcess` event lands after
    /// the suite's load test finishes, `allSuites` is announced once, and a
    /// raw shell line prints exactly as written.
    #[test]
    fn json_merge_replays_machine_events_and_shell_lines() {
        let suite = events(&[
            r#"{"suite":{"id":0,"platform":"vm","path":"test/a_test.dart"},"type":"suite","time":0}"#,
            r#"{"test":{"id":1,"name":"loading test/a_test.dart","suiteID":0,"groupIDs":[],"metadata":{"skip":false,"skipReason":null}},"type":"testStart","time":0}"#,
            r#"{"testID":1,"result":"success","skipped":false,"hidden":true,"type":"testDone","time":1}"#,
        ]);
        let mut out = Vec::new();
        let mut merge = JsonMerge::start(&mut out, None, Instant::now(), 1).expect("start");
        merge.machine = true;
        merge.block(&suite, &mut out).expect("block");
        merge.block(&[crate::event::shell("Shell: from the tool")], &mut out).expect("block");
        merge.done(false, &mut out).expect("done");
        let text = String::from_utf8(out).expect("utf8");
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines[0].starts_with(r#"{"protocolVersion":"0.1.1","runnerVersion":null,"pid":"#));
        assert!(lines[0].ends_with(r#","type":"start","time":0}"#));
        let kinds: Vec<String> = lines.iter().filter_map(|line| parse(line).map(|event| event.kind().to_owned())).collect();
        assert_eq!(kinds, ["start", "suite", "testStart", "allSuites", "testDone", "done"]);
        let started = lines.iter().position(|line| line.contains("test.startedProcess")).expect("started");
        assert_eq!(lines[started], r#"[{"event":"test.startedProcess","params":{"vmServiceUri":null}}]"#);
        assert_eq!(lines[started - 2], r#"{"count":1,"time":1,"type":"allSuites"}"#);
        assert!(lines.contains(&"Shell: from the tool"), "the shell line prints raw");
    }

    /// An error after a test passed takes it back out of the passed count
    /// (the expanded reporter shows `+0 -1` and lists it), and the silent
    /// reporter prints nothing at all.
    #[test]
    fn late_errors_take_a_passed_test_back() {
        let suite = events(&[
            r#"{"suite":{"id":0,"platform":"vm","path":"test/a_test.dart"},"type":"suite","time":0}"#,
            r#"{"test":{"id":1,"name":"loading test/a_test.dart","suiteID":0,"groupIDs":[],"metadata":{"skip":false,"skipReason":null}},"type":"testStart","time":0}"#,
            r#"{"testID":1,"result":"success","skipped":false,"hidden":true,"type":"testDone","time":1}"#,
            r#"{"test":{"id":2,"name":"t","suiteID":0,"groupIDs":[],"metadata":{"skip":false,"skipReason":null}},"type":"testStart","time":1}"#,
            r#"{"testID":2,"result":"success","skipped":false,"hidden":false,"type":"testDone","time":2}"#,
            r#"{"testID":2,"error":"late","stackTrace":"frame","isFailure":true,"type":"error","time":3}"#,
        ]);
        let text = render(Kind::Expanded, &[suite.clone()]);
        assert!(text.contains("00:00 +0: test/a_test.dart: t\n"));
        assert!(text.contains("00:00 +0 -1: test/a_test.dart: t [E]\n"));
        assert!(text.contains("Failing tests:\n  test/a_test.dart: t\n"));
        assert_eq!(render(Kind::Silent, &[suite]), "");
    }
}
