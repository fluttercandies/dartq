//! `harness replay` — the incremental-consistency driver: an edit
//! sequence drives ONE live dartq daemon session (overlay `didChange` +
//! the `dartq/analyze` pull face = the incremental result) while the same
//! edits land on a materialized disk copy re-analyzed fresh per step (the
//! full-recompute result). Per step both diagnostic sets normalize to one
//! canonical shape ([`replay_edit::CanonicalDiag`]) and compare byte-wise;
//! the run passes only when every step matches.
//!
//! Why this shape: the gate demands "incremental result == full-recompute
//! result, except internal IDs". The daemon's `dartq/analyze` pull answers
//! with the post-edit whole-workspace view synchronously (one turn applies
//! queued edits before answering), sidestepping the publish face's
//! change-only baseline suppression; the pull carries no volatile fields
//! (see `replay_edit.rs` for the normalization ledger).
//!
//! Both faces share ONE mutable workspace: the materialized copy. The
//! daemon session runs on the copy (its `{dir}`), and the caller's corpus
//! dir is never written — a crash mid-replay leaves it untouched. Splices
//! ride whole-text `didChange` with the same bytes written to the copy.
//! File-set steps mutate the copy on disk before the matching
//! notification, so both arms observe identical file sets: a remove is the
//! on-disk delete followed by `didClose` — the session stages a removal
//! exactly when its disk read finds the file gone (`Session::queue_close`
//! stages the on-disk bytes over an existing file, which would resurrect
//! it), and it only acts on session-opened documents, so a never-opened
//! target is didOpened first (the staged Set is superseded by the Remove
//! at the same drain). A create is the on-disk write followed by
//! `didOpen`. The per-step byte comparison therefore holds across
//! file-set changes, not just content changes: a removed file's
//! diagnostics vanish from both planes and every importer's churn errors
//! must match line for line. (A file that carries no diagnostics on either
//! face is unobservable on this face by construction — the comparison
//! plane is the diagnostics themselves; universe consistency behind a
//! silent file is pinned at the engine level, not here.)
//!
//! Non-vacuity: an all-equal run means something only if the steps
//! compared anything. A sequence where every step carried zero diagnostics
//! on BOTH faces is the vacuous signature — a universe-narrowing break (a
//! silently skipped file, a filtered-out code family) silences both arms
//! identically and would otherwise pass on empty sets. The report
//! therefore records the per-step and sequence diagnostic totals it
//! compared and fails a whole-sequence zero with an explicit reason, even
//! when every step was equal. Individual clean steps are legitimate; only
//! a sequence-level zero across both arms is judged.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::io::{BufRead, BufReader, Write as IoWrite};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

use rustc_hash::FxHashMap;
use serde_json::{Value, json};

use crate::machine::{MachineDiag, OutputFormat};
use crate::oracle::{TempCapture, driver_timeout_from_env, kill_driver, stderr_tail};
use crate::{Outcome, canonical_corpus, dartq::DartqProbe, replay_edit};

/// LSP frames are JSON bodies; 256 MiB is the daemon codec's own cap.
const MAX_FRAME_BYTES: usize = 256 * 1024 * 1024;

/// Writes one LSP/JSON-RPC frame: `Content-Length` + CRLF blank line +
/// body (byte-identical to the daemon codec's `write_frame`).
pub(crate) fn write_frame(w: &mut impl IoWrite, body: &str) -> std::io::Result<()> {
    write!(w, "Content-Length: {}\r\n\r\n{body}", body.len())
}

/// Reads one framed body. `Ok(None)` = clean EOF before any header (the
/// peer closed); anything else malformed is an error. Header set per the
/// LSP base protocol: `Content-Length` required, other headers ignored.
pub(crate) fn read_frame(reader: &mut impl BufRead) -> Result<Option<String>, String> {
    let mut content_length: Option<usize> = None;
    loop {
        let mut line = Vec::new();
        let read = reader
            .read_until(b'\n', &mut line)
            .map_err(|e| format!("reading frame header: {e}"))?;
        if read == 0 {
            if content_length.is_some() {
                return Err("stream ended inside a frame (headers without a body)".into());
            }
            return Ok(None);
        }
        let line = String::from_utf8(line)
            .map_err(|_| "frame header is not UTF-8".to_string())?
            .trim_end_matches(['\n', '\r'])
            .to_owned();
        if line.is_empty() {
            break;
        }
        if let Some(value) = line.strip_prefix("Content-Length:") {
            let value: usize = value
                .trim()
                .parse()
                .map_err(|_| format!("invalid Content-Length header: {line}"))?;
            if value > MAX_FRAME_BYTES {
                return Err(format!(
                    "Content-Length {value} exceeds the {MAX_FRAME_BYTES} byte cap"
                ));
            }
            content_length = Some(value);
        }
        // Other headers (Content-Type) are legal and ignored.
    }
    let len = content_length.ok_or_else(|| "frame has no Content-Length header".to_string())?;
    let mut body = vec![0u8; len];
    reader
        .read_exact(&mut body)
        .map_err(|e| format!("reading frame body: {e}"))?;
    String::from_utf8(body)
        .map(Some)
        .map_err(|e| format!("frame body is not UTF-8: {e}"))
}

/// Path → `file://` URI, percent-encoding everything outside the URI
/// unreserved set (the daemon's `uri::from_path` counterpart, so the two
/// faces agree on paths containing spaces or non-ASCII).
pub(crate) fn path_to_uri(path: &Path) -> String {
    let text = path.display().to_string();
    let mut uri = String::from("file://");
    for byte in text.as_bytes() {
        match *byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'.' | b'_' | b'~' => {
                uri.push(*byte as char);
            }
            other => write!(uri, "%{other:02X}").expect("string write"),
        }
    }
    uri
}

/// The live-session half: a `dartq daemon` child spoken to over framed
/// stdio. The reader thread owns stdout; requests are answered by matching
/// ids while server notifications (publishDiagnostics) flow past. stderr
/// lands in a temp file for failure tails; teardown is ordered shutdown →
/// exit → reap, with a process-group kill on every other path (Drop
/// included, so an error return can never leak the daemon).
struct DaemonClient {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    frames: std::sync::mpsc::Receiver<Option<String>>,
    stderr: TempCapture,
    next_id: u64,
    clean_shutdown: bool,
}

impl DaemonClient {
    /// `program args` is the resolved `--daemon-cmd` template (`{dir}`
    /// already substituted). The child gets its own process group so a
    /// timed-out call kills the daemon AND its grandchildren (same wedge
    /// avoidance as the oracle driver).
    fn spawn(program: &str, args: &[String]) -> Result<Self, String> {
        let stderr = TempCapture::new();
        let stderr_file = std::fs::File::create(&stderr.stderr_path)
            .map_err(|e| format!("creating daemon stderr capture: {e}"))?;
        let mut cmd = Command::new(program);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::from(stderr_file));
        #[cfg(unix)]
        unsafe {
            use std::os::unix::process::CommandExt;
            // SAFETY: runs in the forked child before exec; setpgid(0, 0)
            // puts the daemon in its own group for kill_driver (see
            // oracle.rs run_captured). A failure is ignored — the fallback
            // kill reaches the direct child only.
            cmd.pre_exec(|| {
                let _ = libc::setpgid(0, 0);
                Ok(())
            });
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("failed to launch the daemon `{program}`: {e}"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "daemon stdout was not piped".to_string())?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "daemon stdin was not piped".to_string())?;
        let (tx, frames) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                // Any decode/IO failure ends the stream: callers report the
                // closed stdout rather than a half-frame.
                if let Ok(Some(frame)) = read_frame(&mut reader) {
                    if tx.send(Some(frame)).is_err() {
                        break;
                    }
                } else {
                    let _ = tx.send(None);
                    break;
                }
            }
        });
        Ok(Self {
            child,
            stdin,
            frames,
            stderr,
            next_id: 1,
            clean_shutdown: false,
        })
    }

    fn stderr_tail(&self) -> String {
        // Decorates error messages; an unreadable capture is surfaced in
        // them, never silently emptied.
        match self.stderr.stderr_bytes() {
            Ok(bytes) => stderr_tail(&bytes),
            Err(e) => format!("<captured stderr unreadable: {e}>"),
        }
    }

    /// One JSON-RPC request → its result, skipping server notifications
    /// until the matching id answers. Budget: `driver_timeout_from_env`,
    /// shared with every other child driver.
    fn call(&mut self, method: &str, params: &Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))?;
        let timeout = driver_timeout_from_env()?;
        let deadline = Instant::now() + timeout;
        loop {
            let now = Instant::now();
            if now >= deadline {
                kill_driver(&mut self.child);
                return Err(format!(
                    "timed out after {}ms waiting for the daemon's `{method}` response; stderr tail:\n{}",
                    timeout.as_millis(),
                    self.stderr_tail()
                ));
            }
            match self.frames.recv_timeout(deadline - now) {
                Ok(Some(frame)) => {
                    let value: Value = serde_json::from_str(&frame)
                        .map_err(|e| format!("daemon sent a non-JSON frame: {e}"))?;
                    if value.get("id").and_then(Value::as_u64) == Some(id) {
                        if let Some(err) = value.get("error") {
                            return Err(format!("daemon returned an error for `{method}`: {err}"));
                        }
                        return Ok(value.get("result").cloned().unwrap_or(Value::Null));
                    }
                    // A server notification (publishDiagnostics, log…):
                    // not ours, keep waiting.
                }
                Ok(None) => {
                    return Err(format!(
                        "daemon stdout closed before the `{method}` response; stderr tail:\n{}",
                        self.stderr_tail()
                    ));
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    kill_driver(&mut self.child);
                    return Err(format!(
                        "timed out after {}ms waiting for the daemon's `{method}` response; stderr tail:\n{}",
                        timeout.as_millis(),
                        self.stderr_tail()
                    ));
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    return Err("daemon reader thread died unexpectedly".into());
                }
            }
        }
    }

    fn send(&mut self, body: &Value) -> Result<(), String> {
        write_frame(&mut self.stdin, &body.to_string())
            .map_err(|e| format!("writing to the daemon: {e}"))
    }

    fn notify(&mut self, method: &str, params: &Value) -> Result<(), String> {
        self.send(&json!({"jsonrpc": "2.0", "method": method, "params": params}))
    }

    /// LSP lifecycle: `initialize` (the daemon gates everything on it) →
    /// `initialized`.
    fn initialize(&mut self) -> Result<(), String> {
        let params = json!({
            "processId": Value::Null,
            "rootUri": Value::Null,
            "capabilities": {},
        });
        self.call("initialize", &params).map(|_| ())?;
        self.notify("initialized", &json!({}))
    }

    /// Ordered teardown; the process-group kill covers every abnormal path.
    fn shutdown(&mut self) -> Result<(), String> {
        let result = self
            .call("shutdown", &Value::Null)
            .and_then(|_| self.notify("exit", &json!({})));
        match result {
            Ok(()) => {
                let waited = self.child.wait().map_err(|e| e.to_string())?;
                self.clean_shutdown = true;
                if waited.success() {
                    Ok(())
                } else {
                    Err(format!(
                        "daemon exited with {waited}; stderr tail:\n{}",
                        self.stderr_tail()
                    ))
                }
            }
            Err(e) => {
                kill_driver(&mut self.child);
                let _ = self.child.wait();
                Err(e)
            }
        }
    }
}

impl Drop for DaemonClient {
    fn drop(&mut self) {
        if !self.clean_shutdown {
            kill_driver(&mut self.child);
            let _ = self.child.wait();
        }
    }
}

/// Workspace diagnostic set in canonical form, ready to compare:
/// (corpus-relative path, canonical diagnostic) pairs, order not yet
/// normalized (see [`canonical_lines`]).
type WorkspaceDiags = Vec<(String, replay_edit::CanonicalDiag)>;

fn relative_key(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}

/// Normalizes a `dartq/analyze` result (the incremental face). `root` is
/// the daemon's workspace dir — the same materialized copy the full
/// face analyzes — so in-corpus paths strip to corpus-relative keys.
fn normalize_lsp_workspace(result: &Value, root: &Path) -> Result<WorkspaceDiags, String> {
    let entries = result
        .get("diagnostics")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            format!(
                "dartq/analyze result carries no `diagnostics` array: {}",
                serde_json::to_string(result).unwrap_or_else(|_| "<unserializable>".into())
            )
        })?;
    let mut out = Vec::with_capacity(entries.len());
    for entry in entries {
        let (path, canonical) = replay_edit::canonical_from_lsp(entry)?;
        out.push((relative_key(&path, root), canonical));
    }
    Ok(out)
}

/// Normalizes a fresh batch machine run (the full-recompute face). Diag
/// files are read once each; positions come from the same file bytes the
/// engine measured (the materialized copy's own content).
fn normalize_machine_workspace(
    diags: &[MachineDiag],
    corpus_root: &Path,
) -> Result<WorkspaceDiags, String> {
    let mut sources: FxHashMap<PathBuf, String> = FxHashMap::default();
    let mut out = Vec::with_capacity(diags.len());
    for diag in diags {
        let file = if diag.file.is_absolute() {
            diag.file.clone()
        } else {
            corpus_root.join(&diag.file)
        };
        let key = relative_key(&file, corpus_root);
        let source = match sources.entry(file) {
            std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::hash_map::Entry::Vacant(entry) => {
                let text = std::fs::read_to_string(entry.key()).map_err(|e| {
                    format!(
                        "reading {} for position normalization: {e}",
                        entry.key().display()
                    )
                })?;
                entry.insert(text)
            }
        };
        let canonical = replay_edit::canonical_from_machine(diag, source)?;
        out.push((key, canonical));
    }
    Ok(out)
}

/// Comparison plane: sorted `file<TAB>key` lines. Byte equality of these
/// vectors IS the verdict for a step.
fn canonical_lines(mut diags: WorkspaceDiags) -> Vec<String> {
    diags.sort();
    diags
        .into_iter()
        .map(|(file, diag)| format!("{file}\t{}", diag.key()))
        .collect()
}

/// First divergence between the two sorted planes, as `(incremental,
/// full)` excerpts; `None` = equal.
fn first_difference(incremental: &[String], full: &[String]) -> Option<(String, String)> {
    let pos = incremental
        .iter()
        .zip(full.iter())
        .position(|(a, b)| a != b)
        .unwrap_or_else(|| incremental.len().min(full.len()));
    if incremental.get(pos) == full.get(pos) {
        None
    } else {
        Some((
            incremental
                .get(pos)
                .cloned()
                .unwrap_or_else(|| "<absent>".into()),
            full.get(pos).cloned().unwrap_or_else(|| "<absent>".into()),
        ))
    }
}

struct StepReport {
    index: usize,
    kind: &'static str,
    file: String,
    line: u32,
    column: u32,
    length: u32,
    equal: bool,
    incremental_count: usize,
    full_count: usize,
    first_diff: Option<(String, String)>,
}

struct ReplayOptions {
    corpus_dir: PathBuf,
    edits_path: PathBuf,
    dartq_cmd: String,
    daemon_cmd: String,
    label: String,
    out_dir: PathBuf,
}

impl Default for ReplayOptions {
    fn default() -> Self {
        Self {
            corpus_dir: PathBuf::new(),
            edits_path: PathBuf::new(),
            dartq_cmd: String::new(),
            daemon_cmd: String::new(),
            label: "replay-r1".into(),
            out_dir: "target/evidence/replay".into(),
        }
    }
}

fn value_arg<'a>(args: &'a [String], i: &mut usize, flag: &str) -> Result<&'a str, Outcome> {
    *i += 1;
    args.get(*i).map(String::as_str).ok_or_else(|| {
        Outcome::Error(format!(
            "--{flag} needs a value; see `harness replay --help`"
        ))
    })
}

fn parse_options(args: &[String]) -> Result<ReplayOptions, Outcome> {
    let mut opts = ReplayOptions::default();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--corpus-dir" => opts.corpus_dir = value_arg(args, &mut i, "corpus-dir")?.into(),
            "--edits" => opts.edits_path = value_arg(args, &mut i, "edits")?.into(),
            "--dartq-cmd" => opts.dartq_cmd = value_arg(args, &mut i, "dartq-cmd")?.to_string(),
            "--daemon-cmd" => opts.daemon_cmd = value_arg(args, &mut i, "daemon-cmd")?.to_string(),
            "--label" => opts.label = value_arg(args, &mut i, "label")?.to_string(),
            "--out" => opts.out_dir = value_arg(args, &mut i, "out")?.into(),
            other => {
                return Err(Outcome::Error(format!(
                    "unknown replay flag `{other}`; see `harness replay --help`"
                )));
            }
        }
        i += 1;
    }
    for (field, flag) in [
        (opts.corpus_dir.as_os_str().is_empty(), "--corpus-dir"),
        (opts.edits_path.as_os_str().is_empty(), "--edits"),
        (opts.dartq_cmd.is_empty(), "--dartq-cmd"),
        (opts.daemon_cmd.is_empty(), "--daemon-cmd"),
    ] {
        if field {
            return Err(Outcome::Error(format!(
                "{flag} is required; see `harness replay --help`"
            )));
        }
    }
    Ok(opts)
}

const USAGE: &str = r#"usage:
  harness replay --corpus-dir DIR --edits FILE.jsonl --dartq-cmd TEMPLATE
                 --daemon-cmd TEMPLATE [--label NAME] [--out DIR]
  Oracle incremental consistency: drive one live `dartq daemon`
  session with an edit sequence (overlay didChange + the `dartq/analyze`
  pull = the incremental result) while the same edits land on a materialized
  disk copy re-analyzed fresh per step (`--dartq-cmd` = the full-recompute
  result). Per step both sides normalize to one canonical diagnostic shape
  and compare byte-wise.
  --edits FILE: JSONL, one step per line, three forms:
      {"file":"lib/a.dart","line":3,"column":8,"length":2,"new_text":"x"}
          in-file splice: 1-based line/column (UTF-16), `length` UTF-16
          code units to replace (0 = insert), `new_text` "" = delete;
          `file` is corpus-relative.
      {"remove":"lib/a.dart"}
          delete the file from the workspace (it must exist).
      {"create":"lib/b.dart","text":"int x = 1;\n"}
          create the file with `text` as its full content (it must not
          exist).
  TEMPLATE: whitespace-split argv; {dir} is replaced by the replay's
  materialized workspace copy (appended as last arg when absent) — both
  faces analyze that same copy, e.g.
      --dartq-cmd "cargo run -q -p dartq-cli -- analyze --format=machine"
      --daemon-cmd "cargo run -q -p dartq-cli -- daemon"
  --label NAME: report directory suffix (default replay-r1). Report:
  <out>/<label>/{report.json,report.md}.
  exit: 0 every step equal and the run non-vacuous | 1 any mismatch, or a
  vacuous all-equal run (zero diagnostics compared across both faces; the
  report names it) | 2 harness error (env, daemon, or malformed edits)."#;

/// Copies a corpus tree to a temp dir, skipping symlinks (same rule as the
/// corpus assembler). Returns the canonicalized copy root so machine-side
/// path stripping stays prefix-stable.
fn materialize_copy(corpus: &Path) -> Result<PathBuf, String> {
    fn copy_tree(src: &Path, dst: &Path) -> Result<(), String> {
        std::fs::create_dir_all(dst).map_err(|e| format!("creating {}: {e}", dst.display()))?;
        for entry in
            std::fs::read_dir(src).map_err(|e| format!("reading {}: {e}", src.display()))?
        {
            let entry = entry.map_err(|e| format!("reading {}: {e}", src.display()))?;
            let file_type = entry
                .file_type()
                .map_err(|e| format!("reading {}: {e}", entry.path().display()))?;
            let target = dst.join(entry.file_name());
            if file_type.is_dir() {
                copy_tree(&entry.path(), &target)?;
            } else if !file_type.is_symlink() {
                std::fs::copy(entry.path(), &target)
                    .map_err(|e| format!("copying {}: {e}", entry.path().display()))?;
            }
        }
        Ok(())
    }
    let dir = std::env::temp_dir().join(format!(
        "dq_replay_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or_default()
    ));
    if dir.exists() {
        std::fs::remove_dir_all(&dir)
            .map_err(|e| format!("clearing stale replay copy {}: {e}", dir.display()))?;
    }
    copy_tree(corpus, &dir)?;
    dir.canonicalize()
        .map_err(|e| format!("canonicalizing the replay copy {}: {e}", dir.display()))
}

/// The run's verdict from the compared step list: every step equal, AND the
/// sequence non-vacuous. An all-equal run that compared zero diagnostics
/// across both faces proves nothing (a universe-narrowing break silences
/// both arms identically), so it fails with the vacuity reason; a run with
/// any mismatch, or with at least one compared diagnostic, keeps its
/// natural verdict.
fn run_verdict(steps: &[StepReport], all_equal: bool) -> (bool, Option<String>) {
    let total_diagnostics: usize = steps
        .iter()
        .map(|step| step.incremental_count + step.full_count)
        .sum();
    if all_equal && total_diagnostics == 0 {
        (
            false,
            Some(
                "vacuous replay: every step compared zero diagnostics on both faces \
                 (incremental and full recompute) — equality over empty sets proves \
                 nothing"
                    .to_owned(),
            ),
        )
    } else {
        (all_equal, None)
    }
}

#[expect(clippy::too_many_lines)] // the JSON and markdown reports render from one step slice; a split would duplicate the schema
fn write_report(
    opts: &ReplayOptions,
    steps: &[StepReport],
    passed: bool,
    failure_reason: Option<&str>,
) -> Result<PathBuf, String> {
    // The step's human-facing description: the splice coordinates read
    // straight off a differential report, the file-set forms name the
    // operation and its target.
    fn step_label(step: &StepReport) -> String {
        if step.kind == "splice" {
            format!("{}:{}+{}", step.file, step.line, step.length)
        } else {
            format!("{} {}", step.kind, step.file)
        }
    }

    let dir = opts.out_dir.join(&opts.label);
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("creating the report dir {}: {e}", dir.display()))?;
    let equal_steps = steps.iter().filter(|s| s.equal).count();
    let total_diagnostics: usize = steps
        .iter()
        .map(|step| step.incremental_count + step.full_count)
        .sum();
    let mut verdict = json!({
           "passed": passed,
           "equal_steps": equal_steps,
    "total_steps": steps.len(),
           "total_diagnostics_compared": total_diagnostics,
       });
    if let Some(reason) = failure_reason {
        verdict["failure_reason"] = json!(reason);
    }
    let json_report = json!({
           "schema": "replay-report-v1",
           "meta": {
               "label": opts.label,
               "corpus_dir": opts.corpus_dir.display().to_string(),
               "edits_file": opts.edits_path.display().to_string(),
               "dartq_cmd": opts.dartq_cmd,
               "daemon_cmd": opts.daemon_cmd,
           },
           "verdict": verdict,
    "steps": steps.iter().map(|step| {
               json!({
                   "index": step.index,
                   "kind": step.kind,
                   "file": step.file,
                   "line": step.line,
                   "column": step.column,
                   "length": step.length,
                   "equal": step.equal,
                   "incremental_diagnostics": step.incremental_count,
                   "full_recompute_diagnostics": step.full_count,
                   "first_diff": step.first_diff.as_ref().map(|(inc, full)| json!({
                       "incremental": inc,
                       "full_recompute": full,
                   })),
               })
           }).collect::<Vec<_>>(),
       });
    let json_path = dir.join("report.json");
    std::fs::write(
        &json_path,
        serde_json::to_string_pretty(&json_report).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("writing {}: {e}", json_path.display()))?;

    let mut md = String::new();
    writeln!(
        md,
        "# harness replay — {}\n\ncorpus: {}\n\nverdict: {} ({equal_steps}/{} steps equal, \
         {total_diagnostics} diagnostics compared)\n\n| step | edit | equal | incremental | full |\n| --- | --- | --- | --- | --- |",
        opts.label,
        opts.corpus_dir.display(),
        if passed { "PASS" } else { "FAIL" },
        steps.len(),
    )
.expect("string write");
    if let Some(reason) = failure_reason {
        writeln!(md, "\n{reason}").expect("string write");
    }
    for step in steps {
        writeln!(
            md,
            "| {} | `{}` | {} | {} | {} |",
            step.index,
            step_label(step),
            if step.equal { "yes" } else { "**no**" },
            step.incremental_count,
            step.full_count,
        )
        .expect("string write");
    }
    if equal_steps < steps.len() {
        md.push_str("\n## Mismatches\n");
        for step in steps.iter().filter(|s| !s.equal) {
            writeln!(
                md,
                "\n### step {} — {}\n\n- incremental: `{}`\n- full: `{}`",
                step.index,
                step_label(step),
                step.first_diff
                    .as_ref()
                    .map_or("<absent>", |(i, _)| i.as_str()),
                step.first_diff
                    .as_ref()
                    .map_or("<absent>", |(_, f)| f.as_str()),
            )
            .expect("string write");
        }
    }
    let md_path = dir.join("report.md");
    std::fs::write(&md_path, md).map_err(|e| format!("writing {}: {e}", md_path.display()))?;
    Ok(dir)
}

/// Resolves a `--daemon-cmd` template to argv, with the same `{dir}`
/// substitution contract as the batch driver (replaced in place, appended
/// when absent).
fn resolve_template(template: &str, dir: &Path) -> (String, Vec<String>) {
    let mut parts = template.split_whitespace().map(str::to_owned);
    let program = parts.next().unwrap_or_default();
    let args: Vec<String> = parts.collect();
    let dir_str = dir.display().to_string();
    let has_placeholder = args.iter().any(|a| a.contains("{dir}"));
    let mut resolved: Vec<String> = args.iter().map(|a| a.replace("{dir}", &dir_str)).collect();
    if !has_placeholder {
        resolved.push(dir_str);
    }
    (program, resolved)
}

/// didOpen one workspace file from its on-disk bytes (the overlay must
/// start from the universe's current content) and track it as opened.
fn open_document(
    daemon: &mut DaemonClient,
    copy: &Path,
    file: &str,
    version: &mut i64,
    opened: &mut BTreeSet<String>,
    texts: &mut BTreeMap<String, String>,
) -> Result<(), String> {
    let path = copy.join(file);
    let text =
        std::fs::read_to_string(&path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    *version += 1;
    daemon.notify(
        "textDocument/didOpen",
        &json!({"textDocument": {
            "uri": path_to_uri(&path),
            "languageId": "dart",
            "version": *version,
            "text": text,
        }}),
    )?;
    opened.insert(file.to_owned());
    texts.insert(file.to_owned(), text);
    Ok(())
}

/// Writes one file's bytes to the copy — the full-recompute face reads the
/// same universe the incremental face's notifications describe.
fn write_copy_file(file: &Path, text: &str, step_no: usize) -> Result<(), String> {
    std::fs::write(file, text)
        .map_err(|e| format!("step {step_no}: writing {}: {e}", file.display()))
}

/// The edit sequence against one daemon session and one materialized copy.
/// The daemon's `{dir}` IS the copy, so every step keeps the two arms on
/// one workspace universe: the notification rides the session overlay, the
/// bytes land on the copy's disk for the fresh per-step batch run.
#[expect(clippy::too_many_lines)] // the step dispatch mirrors the edit-sequence grammar; each arm is one step kind
fn run_steps(
    daemon: &mut DaemonClient,
    batch: &crate::dartq::DartqDriver,
    copy: &Path,
    edits: &[replay_edit::EditStep],
) -> Result<(Vec<StepReport>, bool), String> {
    let mut opened: BTreeSet<String> = BTreeSet::new();
    let mut texts: BTreeMap<String, String> = BTreeMap::new();
    let mut version: i64 = 0;
    let mut steps = Vec::with_capacity(edits.len());
    let mut all_equal = true;
    for (idx, step) in edits.iter().enumerate() {
        let step_no = idx + 1;
        match step {
            replay_edit::EditStep::Splice(edit) => {
                let path = copy.join(&edit.file);
                if !path.exists() {
                    return Err(format!(
                        "step {step_no}: splice targets {}, which is not in the workspace \
                         (absent from the corpus and never created by a `create` step)",
                        edit.file
                    ));
                }
                // Live side: didOpen on first touch (the overlay must start
                // from the on-disk bytes), then the whole-text didChange.
                if !opened.contains(&edit.file) {
                    open_document(
                        daemon,
                        copy,
                        &edit.file,
                        &mut version,
                        &mut opened,
                        &mut texts,
                    )?;
                }
                let new_text = replay_edit::apply_edit(&texts[&edit.file], edit)
                    .map_err(|e| format!("step {step_no}: {e}"))?;
                version += 1;
                let uri = path_to_uri(&path);
                daemon.notify(
                    "textDocument/didChange",
                    &json!({
                                           "textDocument": {"uri": uri, "version": version},
                    "contentChanges": [{"text": new_text.clone()}],
                                       }),
                )?;
                texts.insert(edit.file.clone(), new_text.clone());
                write_copy_file(&path, &new_text, step_no)?;
            }
            replay_edit::EditStep::Remove { file } => {
                let path = copy.join(file);
                if !path.exists() {
                    return Err(format!(
                        "step {step_no}: `remove` targets {file}, which is not part of the \
                         workspace (absent from the corpus and never created); only an existing \
                         file can be removed"
                    ));
                }
                // didClose only acts on session-opened documents (the
                // session no-ops otherwise), so open the file first when
                // this session never touched it; the staged Set is
                // superseded by the Remove at the same drain.
                if !opened.contains(file) {
                    open_document(daemon, copy, file, &mut version, &mut opened, &mut texts)?;
                }
                // The on-disk delete MUST land before the didClose: the
                // session stages a removal only when its disk read finds
                // the file gone — over an existing file it stages the
                // on-disk bytes (a revert), resurrecting the file.
                std::fs::remove_file(&path)
                    .map_err(|e| format!("step {step_no}: removing {}: {e}", path.display()))?;
                daemon.notify(
                    "textDocument/didClose",
                    &json!({"textDocument": {"uri": path_to_uri(&path)}}),
                )?;
                opened.remove(file);
                texts.remove(file);
            }
            replay_edit::EditStep::Create { file, text } => {
                let path = copy.join(file);
                if path.exists() {
                    return Err(format!(
                        "step {step_no}: `create` targets {file}, which already exists (part of \
                         the corpus or created by an earlier step); splice into it or remove it \
                         first"
                    ));
                }
                if let Some(parent) = path.parent()
                    && parent != Path::new("")
                {
                    std::fs::create_dir_all(parent).map_err(|e| {
                        format!("step {step_no}: creating {}: {e}", parent.display())
                    })?;
                }
                write_copy_file(&path, text, step_no)?;
                version += 1;
                daemon.notify(
                    "textDocument/didOpen",
                    &json!({"textDocument": {
                        "uri": path_to_uri(&path),
                        "languageId": "dart",
                        "version": version,
                        "text": text,
                    }}),
                )?;
                opened.insert(file.clone());
                texts.insert(file.clone(), text.clone());
            }
        }

        // Incremental face: the post-step whole-workspace pull.
        let incremental =
            normalize_lsp_workspace(&daemon.call("dartq/analyze", &json!({}))?, copy)?;

        // Full-recompute face: the same universe on disk, analyzed fresh
        // by the batch driver.
        let diags = match batch.probe(copy, OutputFormat::Machine) {
            DartqProbe::Measured(diags) => diags,
            DartqProbe::Panic => {
                return Err(format!(
                    "step {step_no}: the fresh dartq run crashed (exit 4) on the replay copy"
                ));
            }
            DartqProbe::Failed(msg) => {
                return Err(format!("step {step_no}: the fresh dartq run failed: {msg}"));
            }
        };
        let full = normalize_machine_workspace(&diags, copy)?;

        let incremental_lines = canonical_lines(incremental);
        let full_lines = canonical_lines(full);
        let equal = incremental_lines == full_lines;
        let first_diff = if equal {
            None
        } else {
            first_difference(&incremental_lines, &full_lines)
        };
        all_equal &= equal;
        steps.push(StepReport {
            index: step_no,
            kind: step.kind(),
            file: step.file().to_owned(),
            line: match step {
                replay_edit::EditStep::Splice(edit) => edit.line,
                _ => 0,
            },
            column: match step {
                replay_edit::EditStep::Splice(edit) => edit.column,
                _ => 0,
            },
            length: match step {
                replay_edit::EditStep::Splice(edit) => edit.length,
                _ => 0,
            },
            equal,
            incremental_count: incremental_lines.len(),
            full_count: full_lines.len(),
            first_diff,
        });
    }
    Ok((steps, all_equal))
}

/// Entry point: preflight → materialized copy → one daemon session over
/// the edit sequence → per-step byte compare → report. `Ok((summary,
/// true))` = every step equal and the run non-vacuous (exit 0);
/// `Ok((_, false))` = at least one mismatch, or a vacuous all-equal run
/// that compared zero diagnostics across both faces (exit 1, the report
/// names which); `Err(Outcome::Error)` = harness/environment error
/// (exit 2): malformed edits, daemon launch/handshake/answer failures,
/// fresh-run crashes.
pub(crate) fn replay_entry(args: &[String]) -> Result<(String, bool), Outcome> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        return Ok((USAGE.trim_end().to_string(), true));
    }
    let opts = parse_options(args)?;
    let corpus = canonical_corpus(&opts.corpus_dir)?;
    let edits_text = std::fs::read_to_string(&opts.edits_path).map_err(|e| {
        Outcome::Error(format!(
            "reading --edits {}: {e}",
            opts.edits_path.display()
        ))
    })?;
    let edits = replay_edit::parse_edits(&edits_text).map_err(|e| {
        Outcome::Error(format!(
            "parsing --edits {}: {e}",
            opts.edits_path.display()
        ))
    })?;
    if edits.is_empty() {
        return Err(Outcome::Error(format!(
            "--edits {} carries no steps; nothing to replay",
            opts.edits_path.display()
        )));
    }
    let copy = materialize_copy(&corpus).map_err(Outcome::Error)?;
    let batch = crate::dartq::DartqDriver::from_template(&opts.dartq_cmd);
    // The daemon session runs on the materialized copy — the one workspace
    // both faces observe, and the only place a remove/create can mutate
    // the file set (the corpus dir is never written).
    let (program, daemon_args) = resolve_template(&opts.daemon_cmd, &copy);
    let mut daemon = DaemonClient::spawn(&program, &daemon_args).map_err(Outcome::Error)?;
    let result = (|| -> Result<(Vec<StepReport>, bool), String> {
        daemon.initialize()?;
        run_steps(&mut daemon, &batch, &copy, &edits)
    })();
    let shutdown_result = daemon.shutdown();
    let _ = std::fs::remove_dir_all(&copy);
    // Step errors win over shutdown errors (the diagnosis points earlier).
    let (steps, all_equal) = result.map_err(Outcome::Error)?;
    shutdown_result.map_err(Outcome::Error)?;
    let (passed, failure_reason) = run_verdict(&steps, all_equal);
    let dir =
        write_report(&opts, &steps, passed, failure_reason.as_deref()).map_err(Outcome::Error)?;
    let equal_steps = steps.iter().filter(|step| step.equal).count();
    let summary = match failure_reason {
        Some(reason) => format!(
            "replay {}: {}/{} steps equal, FAIL — {reason}; report: {}",
            opts.label,
            equal_steps,
            steps.len(),
            dir.display(),
        ),
        None => format!(
            "replay {}: {}/{} steps equal ({}); report: {}",
            opts.label,
            equal_steps,
            steps.len(),
            if passed { "PASS" } else { "FAIL" },
            dir.display(),
        ),
    };
    Ok((summary, passed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine;

    #[test]
    fn frames_round_trip_and_stack() {
        let mut buffer = Vec::new();
        write_frame(&mut buffer, "{\"a\":1}").expect("write");
        write_frame(&mut buffer, "second").expect("write");
        let mut reader = std::io::Cursor::new(buffer);
        assert_eq!(
            read_frame(&mut reader).expect("first frame"),
            Some("{\"a\":1}".to_string())
        );
        assert_eq!(
            read_frame(&mut reader).expect("second frame"),
            Some("second".to_string())
        );
        assert_eq!(read_frame(&mut reader).expect("eof"), None);
    }

    #[test]
    fn frames_reject_malformed_input() {
        let mut reader = std::io::Cursor::new(b"Content-Type: text\r\n\r\n{}".to_vec());
        assert!(
            read_frame(&mut reader)
                .unwrap_err()
                .contains("Content-Length"),
            "a frame without Content-Length must be rejected"
        );
        let mut reader = std::io::Cursor::new(b"Content-Length: 100\r\n\r\nshort".to_vec());
        assert!(
            read_frame(&mut reader)
                .unwrap_err()
                .contains("reading frame body"),
            "a truncated body must be rejected"
        );
        let mut reader = std::io::Cursor::new(b"Content-Length: nope\r\n\r\n".to_vec());
        assert!(
            read_frame(&mut reader)
                .unwrap_err()
                .contains("invalid Content-Length"),
            "a non-numeric length must be rejected"
        );
    }

    #[test]
    fn uris_round_trip_through_percent_encoding() {
        for path in [
            "/tmp/plain/lib/a.dart",
            "/tmp/with space/a.dart",
            "/tmp/uni\u{4e2d}\u{6587}/a.dart",
            "relative/corpus/lib/b.dart",
        ] {
            let uri = path_to_uri(Path::new(path));
            assert!(
                uri.starts_with("file://"),
                "{path} must produce a file URI, got {uri}"
            );
            assert_eq!(
                replay_edit::file_uri_to_path(&uri).expect("decode"),
                PathBuf::from(path),
                "uri round trip failed for {path}"
            );
        }
    }

    #[test]
    fn lsp_workspace_normalizes_with_corpus_relative_keys() {
        let result = json!({
            "schema_version": 1,
            "diagnostics": [
                {"uri": "file:///corpus/lib/a.dart", "range": {"start": {"line": 1, "character": 4}, "end": {"line": 1, "character": 7}}, "severity": 1, "code": "expected_token", "source": "dartq", "message": "Expected ';'."},
                {"uri": "file:///elsewhere/pub/cache/x.dart", "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}}, "severity": 2, "code": "Some_Code", "source": "dartq", "message": "m"},
            ],
        });
        let diags = normalize_lsp_workspace(&result, Path::new("/corpus")).expect("normalizes");
        assert_eq!(diags.len(), 2);
        assert_eq!(diags[0].0, "lib/a.dart", "in-corpus path becomes relative");
        assert_eq!(
            diags[1].0, "/elsewhere/pub/cache/x.dart",
            "out-of-corpus path stays absolute (visible, not dropped)"
        );
        assert_eq!(diags[1].1.code, "some_code", "codes normalize to lowercase");
    }

    #[test]
    fn machine_workspace_normalizes_against_copy_files() {
        let dir = std::env::temp_dir().join(format!("dq_replay_test_{}", std::process::id()));
        std::fs::create_dir_all(dir.join("lib")).expect("mkdir");
        std::fs::write(dir.join("lib/a.dart"), "int a = 1;\nint b = 2;\n").expect("fixture");
        let diags = vec![MachineDiag {
            severity: machine::Severity::Error,
            dtype: "SYNTACTIC_ERROR".into(),
            code: "EXPECTED_TOKEN".into(),
            file: dir.join("lib/a.dart"),
            line: 2,
            col: 5,
            length: 3,
            message: "Expected ';'.".into(),
        }];
        let normalized = normalize_machine_workspace(&diags, &dir).expect("normalizes");
        assert_eq!(normalized.len(), 1);
        assert_eq!(normalized[0].0, "lib/a.dart");
        assert_eq!(
            normalized[0].1.key(),
            replay_edit::canonical_from_lsp(&json!({
                "uri": format!("file://{}", path_to_uri(&dir.join("lib/a.dart"))),
                "range": {"start": {"line": 1, "character": 4}, "end": {"line": 1, "character": 7}},
                "severity": 1, "code": "expected_token", "source": "dartq", "message": "Expected ';'.",
            }))
            .expect("lsp")
            .1
            .key(),
            "both faces must land on one canonical key"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn first_difference_reports_both_sides_or_none() {
        assert_eq!(first_difference(&["a".into()], &["a".into()]), None);
        assert_eq!(first_difference(&[], &[]), None);
        let (inc, full) = first_difference(&["a".into(), "b".into()], &["a".into(), "c".into()])
            .expect("differs");
        assert_eq!((inc.as_str(), full.as_str()), ("b", "c"));
        let (inc, full) = first_difference(&["a".into()], &[]).expect("length divergence");
        assert_eq!((inc.as_str(), full.as_str()), ("a", "<absent>"));
    }

    #[test]
    fn options_parse_flags_and_defaults() {
        let parsed = parse_options(&[
            "--corpus-dir".into(),
            "c".into(),
            "--edits".into(),
            "e.jsonl".into(),
            "--dartq-cmd".into(),
            "dq analyze".into(),
            "--daemon-cmd".into(),
            "dq daemon".into(),
        ])
        .expect("parses");
        assert_eq!(parsed.label, "replay-r1", "default label");
        assert_eq!(parsed.out_dir, PathBuf::from("target/evidence/replay"));
        assert!(
            parse_options(&["--corpus-dir".into(), "c".into()]).is_err(),
            "missing required flags must fail"
        );
        assert!(
            parse_options(&["--nope".into()]).is_err(),
            "unknown flags must fail"
        );
    }

    fn verdict_step(incremental: usize, full: usize, equal: bool) -> StepReport {
        StepReport {
            index: 1,
            kind: "splice",
            file: "lib/a.dart".into(),
            line: 1,
            column: 1,
            length: 0,
            equal,
            incremental_count: incremental,
            full_count: full,
            first_diff: None,
        }
    }

    #[test]
    fn vacuous_all_equal_sequence_fails_the_verdict() {
        // The vacuous signature: all steps equal, zero diagnostics on both
        // faces across the whole sequence.
        let (passed, reason) = run_verdict(&[verdict_step(0, 0, true)], true);
        assert!(!passed, "an all-equal zero-diagnostics run must fail");
        let reason = reason.expect("the vacuity failure is named");
        assert!(
            reason.contains("vacuous"),
            "reason names the vacuity: {reason}"
        );

        // A clean step among comparing steps is legitimate, not vacuous.
        let (passed, reason) =
            run_verdict(&[verdict_step(0, 0, true), verdict_step(2, 2, true)], true);
        assert!(passed, "a sequence that compared anything passes");
        assert!(reason.is_none());

        // A mismatch fails regardless of the totals.
        let (passed, reason) = run_verdict(&[verdict_step(0, 0, false)], false);
        assert!(!passed);
        assert!(reason.is_none(), "a plain mismatch needs no vacuity reason");
    }
}
