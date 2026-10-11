//! dartq-side panic isolation (ticket #310): a dartq crash (exit 4 with empty
//! stdout) kills the whole batch — `catch_unwind` wraps every corpus file, so
//! one panicking file turns the entire run into a bare harness error with no
//! attribution. This module is a harness-side attribution layer: it re-runs
//! the dartq command over halves of the corpus file set until the panicking
//! files are isolated.
//!
//! The gate is untouched: a panicking run still fails as a harness error and
//! never produces a verdict — only the failure now carries the attributed file
//! list (console + `dartq_panic_files` in the attribution report).
//!
//! Mechanics: probe subsets via symlink farms in a temp session dir (the
//! `.dart` payload is symlinked, never copied; the package scaffolding —
//! pubspec, package config, pubspec lock — is copied so tooling writes cannot
//! leak into the real corpus). Probe subsets are strictly smaller than their
//! parent set, so the walk structurally terminates; a rerun budget
//! (`2 * ceil(log2(N)) + 4`) fuses runaway attribution into a PARTIAL result.
//! POSIX-only (symlink farms); Windows is out of scope.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::corpus;
use crate::dartq::{DartqDriver, DartqProbe};
use crate::machine::{MachineDiag, OutputFormat};

/// Result of the dartq analysis phase across all corpus dirs.
#[derive(Debug)]
pub(crate) enum DartqPhaseError {
    /// A non-panic driver failure (launch error, timeout, usage exit,
    /// malformed output) — byte-identical to the pre-isolation harness error.
    Hard(String),
    /// At least one corpus dir hit the crash tier; attribution attached.
    Panicked(PanicAttribution),
}

/// Attribution for one panicking run (possibly spanning several corpus dirs).
#[derive(Debug, Default)]
pub(crate) struct PanicAttribution {
    /// Attributed panicking files: (corpus label, corpus-relative path);
    /// label is `None` for a single-corpus run.
    pub(crate) entries: Vec<(Option<String>, String)>,
    /// `true` = the rerun budget ran out (or a rerun failed off-signature)
    /// before every failing subset was resolved: the list may be incomplete.
    pub(crate) partial: bool,
    /// Total dartq rerun invocations spent bisecting (probes).
    pub(crate) probes: usize,
    /// Non-fatal observations that qualify the attribution.
    pub(crate) notes: Vec<String>,
}

impl PanicAttribution {
    /// Console face: one line per corpus dir with its attributed files.
    pub(crate) fn summary_lines(&self) -> String {
        if self.entries.is_empty() {
            return "  (no panicking file attributed)".to_string();
        }
        let mut groups: Vec<(Option<String>, Vec<&str>)> = Vec::new();
        for (label, file) in &self.entries {
            match groups.iter_mut().find(|(l, _)| l == label) {
                Some((_, files)) => files.push(file),
                None => groups.push((label.clone(), vec![file])),
            }
        }
        groups
            .into_iter()
            .map(|(label, files)| {
                format!(
                    "  {}: {}",
                    label.as_deref().unwrap_or("(default corpus)"),
                    files.join(", ")
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Full harness-error text: headline with counts, the per-corpus file
    /// list, the not-judged marker, the report location, and any notes that
    /// qualify the attribution (partial budget, non-reproducing crash).
    pub(crate) fn error_message(&self, report_path: &Path) -> String {
        let mut msg = format!(
            "dartq crashed (exit 4, empty stdout); corpus auto-bisect attributed {} \
             panicking file(s):\n{}\ngate not judged (the run still fails as a harness \
             error); attribution report: {}",
            self.entries.len(),
            self.summary_lines(),
            report_path.display(),
        );
        if self.partial {
            msg.push_str("\nattribution is PARTIAL: rerun budget exhausted");
        }
        if !self.notes.is_empty() {
            let _ = write!(msg, "\nnotes: {}", self.notes.join("; "));
        }
        msg
    }
}

/// Run the dartq driver once per corpus dir, isolating crash-tier dirs via
/// auto-bisect. Ok = every dir measured. Hard = a non-panic driver failure
/// (the historical error text). Panicked = at least one dir crashed; the
/// caller reports the attribution and the run still fails as a harness error.
pub(crate) fn dartq_phase(
    corpora: &[(Option<String>, PathBuf)],
    driver: &DartqDriver,
    format: OutputFormat,
) -> Result<Vec<Vec<MachineDiag>>, DartqPhaseError> {
    let mut measured = Vec::new();
    let mut attribution = PanicAttribution::default();
    let mut panicked = false;
    for (label, dir) in corpora {
        match driver.probe(dir, format) {
            DartqProbe::Measured(diags) => measured.push(diags),
            DartqProbe::Failed(msg) => return Err(DartqPhaseError::Hard(msg)),
            DartqProbe::Panic => {
                panicked = true;
                attribute_panic(driver, format, label.as_deref(), dir, &mut attribution)
                    .map_err(DartqPhaseError::Hard)?;
            }
        }
    }
    if panicked {
        return Err(DartqPhaseError::Panicked(attribution));
    }
    Ok(measured)
}

/// Bisect one crashed corpus dir and merge the found files into `attribution`.
fn attribute_panic(
    driver: &DartqDriver,
    format: OutputFormat,
    label: Option<&str>,
    corpus_dir: &Path,
    attribution: &mut PanicAttribution,
) -> Result<(), String> {
    let files = corpus::dart_files(corpus_dir)?;
    let mut session = Bisection::new(driver, format, corpus_dir);
    session.run(&files);
    attribution.partial |= session.partial;
    attribution.probes += session.probes;
    // `Bisection` drops its session dir; take the findings out first.
    attribution.notes.extend(std::mem::take(&mut session.notes));
    let found = std::mem::take(&mut session.found);
    attribution.entries.extend(
        found
            .into_iter()
            .map(|file| (label.map(str::to_string), file)),
    );
    Ok(())
}

/// Halving walk over one crashed corpus dir. Invariant: `walk` is only called
/// on file sets already proven to panic (the full set via the crashed batch
/// run, subsets via a failing probe) — a singleton reaching `walk` is
/// attributed without another probe.
struct Bisection<'a> {
    driver: &'a DartqDriver,
    format: OutputFormat,
    corpus_root: PathBuf,
    session_dir: PathBuf,
    next_probe_id: u64,
    remaining_budget: u32,
    probes: usize,
    found: Vec<String>,
    partial: bool,
    notes: Vec<String>,
}

static NEXT_SESSION_ID: AtomicU64 = AtomicU64::new(0);

impl Drop for Bisection<'_> {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.session_dir).ok();
    }
}

impl<'a> Bisection<'a> {
    fn new(driver: &'a DartqDriver, format: OutputFormat, corpus_dir: &Path) -> Self {
        let id = NEXT_SESSION_ID.fetch_add(1, Ordering::Relaxed);
        Self {
            driver,
            format,
            corpus_root: corpus_dir.to_path_buf(),
            session_dir: std::env::temp_dir().join(format!(
                "dq_bisect_{}_{}",
                std::process::id(),
                id
            )),
            next_probe_id: 0,
            remaining_budget: 0,
            probes: 0,
            found: Vec::new(),
            partial: false,
            notes: Vec::new(),
        }
    }

    /// Rerun budget: a single panicking file costs ~2 probes per halving level
    /// (`2 * ceil(log2(N))`); `+4` absorbs the reproducing root probe and
    /// last-level asymmetries. Multiple panicking files may exhaust it — that
    /// degrades to PARTIAL attribution, never to runaway rerunning.
    fn run(&mut self, files: &[PathBuf]) {
        if files.is_empty() {
            self.notes
                .push("corpus dir contains no .dart files; nothing to bisect".into());
            return;
        }
        self.remaining_budget = 2 * ceil_log2(files.len()) + 4;
        match self.probe(files) {
            DartqProbe::Panic => self.walk(files),
            DartqProbe::Measured(_) => self
                .notes
                .push("panic did not reproduce in the isolated rerun; no file attributed".into()),
            DartqProbe::Failed(msg) => self
                .notes
                .push(format!("isolated rerun failed off-signature: {msg}")),
        }
    }

    fn walk(&mut self, files: &[PathBuf]) {
        if files.len() == 1 {
            self.found.push(files[0].display().to_string());
            return;
        }
        let mid = files.len() / 2;
        let halves: [&[PathBuf]; 2] = [&files[..mid], &files[mid..]];
        for half in halves {
            if self.remaining_budget == 0 {
                self.partial = true;
                return;
            }
            match self.probe(half) {
                DartqProbe::Measured(_) => {}
                DartqProbe::Panic => self.walk(half),
                DartqProbe::Failed(msg) => {
                    self.partial = true;
                    self.notes.push(format!(
                        "bisect rerun over {} file(s) failed off-signature: {msg}",
                        half.len()
                    ));
                }
            }
        }
    }

    /// One dartq rerun over a subset, via a fresh symlink farm. Callers must
    /// budget-check first; every call spends one probe.
    fn probe(&mut self, files: &[PathBuf]) -> DartqProbe {
        self.probes += 1;
        self.remaining_budget -= 1;
        let dir = self
            .session_dir
            .join(format!("probe_{}", self.next_probe_id));
        self.next_probe_id += 1;
        if let Err(e) = build_subset(&self.corpus_root, files, &dir) {
            return DartqProbe::Failed(format!("building subset farm: {e}"));
        }
        self.driver.probe(&dir, self.format)
    }
}

/// Total reruns allowed for a candidate set of `n` files (see [`Bisection::run`]).
fn ceil_log2(n: usize) -> u32 {
    usize::BITS - (n - 1).leading_zeros()
}

/// Materialize a probe subset at `dest`: the `.dart` payload as symlinks into
/// the real corpus (zero content copying), the package scaffolding (pubspec +
/// package config + pubspec lock) copied so tooling writes cannot leak into
/// the real corpus — and so dartq's workspace resolution answers here exactly
/// as it answers on the corpus (a probe without the lock trips dartq's usage
/// exit instead of the signature under attribution, #567).
fn build_subset(corpus_root: &Path, files: &[PathBuf], dest: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dest).map_err(|e| format!("creating {}: {e}", dest.display()))?;
    for scaffold in [
        "pubspec.yaml",
        ".dart_tool/package_config.json",
        "pubspec.lock",
    ] {
        let src = corpus_root.join(scaffold);
        if src.is_file() {
            let target = dest.join(scaffold);
            ensure_parent(&target)?;
            std::fs::copy(&src, &target).map_err(|e| format!("copying {}: {e}", src.display()))?;
        }
    }
    for rel in files {
        let target = dest.join(rel);
        ensure_parent(&target)?;
        std::os::unix::fs::symlink(corpus_root.join(rel), &target)
            .map_err(|e| format!("linking {}: {e}", target.display()))?;
    }
    Ok(())
}

fn ensure_parent(path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("creating {}: {e}", parent.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Mini corpus package: `lib/<name>.dart` per name + pubspec + package
    /// config (languageVersion >= 3.13, the corpus contract).
    fn write_corpus(root: &Path, dart_names: &[&str], extra: &[(&str, &str)]) -> PathBuf {
        for name in dart_names {
            let path = root.join("lib").join(format!("{name}.dart"));
            fs::create_dir_all(path.parent().unwrap()).expect("mkdir");
            fs::write(path, format!("void {name}() {{}}\n")).expect("dart file");
        }
        for (rel, content) in extra {
            let path = root.join(rel);
            fs::create_dir_all(path.parent().unwrap()).expect("mkdir");
            fs::write(path, content).expect("extra file");
        }
        root.to_path_buf()
    }

    const SCAFFOLD_PUBSPEC: &str = "name: bisect_mini\n";
    const SCAFFOLD_CONFIG: &str = r#"{"configVersion":2,"packages":[{"name":"c","rootUri":"../","packageUri":"lib/","languageVersion":"3.13"}],"generator":"pub"}"#;

    /// Mock dartq driver (the `--dartq-cmd` template seam): exits 4 with empty
    /// stdout iff the analyzed dir contains one of `panic_targets`, else
    /// exits 0 (a clean measurement).
    fn write_panic_script(dir: &Path, panic_targets: &[&str]) -> String {
        use std::os::unix::fs::PermissionsExt;

        let script = dir.join("mock_dartq.sh");
        let body = format!(
            "#!/bin/sh\nfor t in {targets}; do\n  if [ -e \"$1/$t\" ]; then\n    exit 4\n  fi\ndone\nexit 0\n",
            targets = panic_targets.join(" "),
        );
        fs::write(&script, body).expect("script");
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).expect("chmod");
        script.display().to_string()
    }

    fn template_for(script: &str) -> DartqDriver {
        DartqDriver::from_template(&format!("{script} {{dir}}"))
    }

    /// End to end: a 6-file corpus with one panicking file bisects to exactly
    /// that file (5 reruns: root + 2 probes per halving level).
    #[test]
    fn bisect_isolates_single_panic_file_end_to_end() {
        let root = std::env::temp_dir().join(format!("dq_bisect_one_{}", std::process::id()));
        fs::create_dir_all(&root).expect("mkdir");
        let corpus_dir = write_corpus(
            &root.join("corpus"),
            &["a", "b", "c", "d", "e", "f"],
            &[
                ("pubspec.yaml", SCAFFOLD_PUBSPEC),
                (".dart_tool/package_config.json", SCAFFOLD_CONFIG),
            ],
        );
        let script = write_panic_script(&root, &["lib/d.dart"]);
        let driver = template_for(&script);

        let err = dartq_phase(
            &[(None, corpus_dir.clone())],
            &driver,
            OutputFormat::Machine,
        )
        .expect_err("panicking corpus must fail the phase");
        let DartqPhaseError::Panicked(attribution) = err else {
            panic!("expected Panicked, got {err:?}");
        };
        assert_eq!(
            attribution.entries,
            vec![(None, "lib/d.dart".to_string())],
            "exactly the panicking file"
        );
        assert!(!attribution.partial, "single panic bisects within budget");
        assert_eq!(attribution.probes, 5, "root + 2 per halving level");
        assert!(attribution.notes.is_empty());
        assert_eq!(
            attribution.summary_lines(),
            "  (default corpus): lib/d.dart"
        );

        fs::remove_dir_all(&root).ok();
    }

    /// Multiple panicking files: both are attributed, budget intact.
    #[test]
    fn bisect_finds_multiple_panic_files() {
        let root = std::env::temp_dir().join(format!("dq_bisect_multi_{}", std::process::id()));
        fs::create_dir_all(&root).expect("mkdir");
        let corpus_dir = write_corpus(
            &root.join("corpus"),
            &["a", "b", "c", "d"],
            &[
                ("pubspec.yaml", SCAFFOLD_PUBSPEC),
                (".dart_tool/package_config.json", SCAFFOLD_CONFIG),
            ],
        );
        let script = write_panic_script(&root, &["lib/a.dart", "lib/d.dart"]);
        let driver = template_for(&script);

        let DartqPhaseError::Panicked(attribution) =
            dartq_phase(&[(None, corpus_dir)], &driver, OutputFormat::Machine)
                .expect_err("panicking corpus must fail the phase")
        else {
            panic!("expected Panicked");
        };
        assert_eq!(
            attribution
                .entries
                .into_iter()
                .map(|(_, f)| f)
                .collect::<Vec<_>>(),
            vec!["lib/a.dart".to_string(), "lib/d.dart".to_string()],
        );
        assert!(!attribution.partial);
        assert_eq!(attribution.probes, 7, "root + full failing subtree");

        fs::remove_dir_all(&root).ok();
    }

    /// All files panicking blows the budget: the fuse cuts the walk and the
    /// attribution degrades to PARTIAL, never to runaway rerunning.
    #[test]
    fn rerun_budget_fuses_into_partial_attribution() {
        let root = std::env::temp_dir().join(format!("dq_bisect_fuse_{}", std::process::id()));
        fs::create_dir_all(&root).expect("mkdir");
        let names: Vec<String> = (0..16).map(|i| format!("f{i:02}")).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let corpus_dir = write_corpus(
            &root.join("corpus"),
            &refs,
            &[
                ("pubspec.yaml", SCAFFOLD_PUBSPEC),
                (".dart_tool/package_config.json", SCAFFOLD_CONFIG),
            ],
        );
        let targets: Vec<String> = names.iter().map(|n| format!("lib/{n}.dart")).collect();
        let target_refs: Vec<&str> = targets.iter().map(String::as_str).collect();
        let script = write_panic_script(&root, &target_refs);
        let driver = template_for(&script);

        let DartqPhaseError::Panicked(attribution) =
            dartq_phase(&[(None, corpus_dir)], &driver, OutputFormat::Machine)
                .expect_err("panicking corpus must fail the phase")
        else {
            panic!("expected Panicked");
        };
        let budget = 2 * ceil_log2(16) + 4;
        assert!(attribution.partial, "16 panics exceed the budget");
        assert_eq!(attribution.probes, budget as usize, "fuse stops at budget");
        assert!(
            !attribution.entries.is_empty(),
            "partial attribution still names the files found so far"
        );

        fs::remove_dir_all(&root).ok();
    }

    /// A crash that does not reproduce on isolated subsets (e.g. state beyond
    /// the file set) attributes nothing but explains itself in the notes.
    #[test]
    fn non_reproducing_panic_yields_note_and_no_entries() {
        let root = std::env::temp_dir().join(format!("dq_bisect_norep_{}", std::process::id()));
        fs::create_dir_all(&root).expect("mkdir");
        // The mock panics on `notes.txt` — a non-.dart file that never enters
        // the candidate set, so no subset can reproduce the crash.
        let corpus_dir = write_corpus(
            &root.join("corpus"),
            &["a", "b"],
            &[
                ("pubspec.yaml", SCAFFOLD_PUBSPEC),
                (".dart_tool/package_config.json", SCAFFOLD_CONFIG),
                ("notes.txt", "not a candidate\n"),
            ],
        );
        let script = write_panic_script(&root, &["notes.txt"]);
        let driver = template_for(&script);

        let DartqPhaseError::Panicked(attribution) =
            dartq_phase(&[(None, corpus_dir)], &driver, OutputFormat::Machine)
                .expect_err("panicking corpus must fail the phase")
        else {
            panic!("expected Panicked");
        };
        assert!(attribution.entries.is_empty());
        assert!(!attribution.partial);
        assert_eq!(
            attribution.summary_lines(),
            "  (no panicking file attributed)"
        );
        assert_eq!(attribution.notes.len(), 1);
        assert!(attribution.notes[0].contains("did not reproduce"));

        fs::remove_dir_all(&root).ok();
    }

    /// Labeled multi-corpus runs keep the corpus label on attributed files;
    /// clean corpora pass through untouched.
    #[test]
    fn labels_ride_attributions_and_clean_dirs_pass_through() {
        let root = std::env::temp_dir().join(format!("dq_bisect_labels_{}", std::process::id()));
        fs::create_dir_all(&root).expect("mkdir");
        let clean = write_corpus(
            &root.join("clean"),
            &["x"],
            &[
                ("pubspec.yaml", SCAFFOLD_PUBSPEC),
                (".dart_tool/package_config.json", SCAFFOLD_CONFIG),
            ],
        );
        let poisoned = write_corpus(
            &root.join("poisoned"),
            &["y"],
            &[
                ("pubspec.yaml", SCAFFOLD_PUBSPEC),
                (".dart_tool/package_config.json", SCAFFOLD_CONFIG),
            ],
        );
        let script = write_panic_script(&root, &["lib/y.dart"]);
        let driver = template_for(&script);

        // Clean run: no isolation, one empty measurement per dir.
        let diags = dartq_phase(
            &[(Some("clean".into()), clean.clone())],
            &driver,
            OutputFormat::Machine,
        )
        .expect("clean corpus measures");
        assert_eq!(diags.len(), 1);
        assert!(diags[0].is_empty());

        // Poisoned dir among two: label rides the entry, clean dir measured.
        let err = dartq_phase(
            &[
                (Some("a".into()), clean.clone()),
                (Some("b".into()), poisoned),
            ],
            &driver,
            OutputFormat::Machine,
        )
        .expect_err("panicking corpus must fail the phase");
        let DartqPhaseError::Panicked(attribution) = err else {
            panic!("expected Panicked");
        };
        assert_eq!(
            attribution.entries,
            vec![(Some("b".to_string()), "lib/y.dart".to_string())]
        );

        fs::remove_dir_all(&root).ok();
    }

    /// The probe project carries the corpus's package scaffolding, the
    /// pubspec lock included (#567): the r3 gate's probes lacked the lock and
    /// dartq answered its usage exit (64, "no pubspec.lock") instead of the
    /// panic signature, so every probe failed off-signature and the
    /// attribution chain died. The scaffold batch is copy-if-present: a
    /// corpus without a lock still builds a probe (and the lock rides as a
    /// copy, never a symlink — tooling writes must not leak into the corpus).
    #[test]
    fn probe_subset_carries_the_package_scaffolding_including_the_lock() {
        let root = std::env::temp_dir().join(format!("dq_bisect_scaffold_{}", std::process::id()));
        fs::create_dir_all(&root).expect("mkdir");
        let corpus = write_corpus(
            &root.join("corpus"),
            &["a", "b"],
            &[
                ("pubspec.yaml", SCAFFOLD_PUBSPEC),
                (".dart_tool/package_config.json", SCAFFOLD_CONFIG),
                ("pubspec.lock", "packages:\n  c:\n    dependency: direct\n"),
            ],
        );
        let dest = root.join("probe_0");
        build_subset(&corpus, &[PathBuf::from("lib/a.dart")], &dest).expect("subset builds");

        for scaffold in [
            "pubspec.yaml",
            ".dart_tool/package_config.json",
            "pubspec.lock",
        ] {
            assert!(
                dest.join(scaffold).is_file(),
                "the probe carries {scaffold}"
            );
        }
        // The scaffolding rides as copies, the payload as symlinks.
        let lock = fs::symlink_metadata(dest.join("pubspec.lock")).expect("lock");
        assert!(!lock.file_type().is_symlink(), "the lock is a copy");
        let payload = fs::symlink_metadata(dest.join("lib/a.dart")).expect("payload");
        assert!(
            payload.file_type().is_symlink(),
            "dart files stay symlinked"
        );

        // A corpus that has not had `dart pub get` run in it (no lock yet)
        // still builds: every present scaffold copies, the absent lock is
        // simply skipped.
        let bare = write_corpus(
            &root.join("bare"),
            &["a"],
            &[
                ("pubspec.yaml", SCAFFOLD_PUBSPEC),
                (".dart_tool/package_config.json", SCAFFOLD_CONFIG),
            ],
        );
        let bare_dest = root.join("probe_bare");
        build_subset(&bare, &[PathBuf::from("lib/a.dart")], &bare_dest)
            .expect("bare subset builds");
        assert!(bare_dest.join("pubspec.yaml").is_file());
        assert!(bare_dest.join(".dart_tool/package_config.json").is_file());
        assert!(!bare_dest.join("pubspec.lock").exists());

        fs::remove_dir_all(&root).ok();
    }

    /// Non-panic driver failures (usage exit 64) pass through as Hard with the
    /// historical error text — no bisect, no attribution.
    #[test]
    fn hard_failures_pass_through_without_bisect() {
        let root = std::env::temp_dir().join(format!("dq_bisect_hard_{}", std::process::id()));
        fs::create_dir_all(&root).expect("mkdir");
        let corpus_dir = write_corpus(
            &root.join("corpus"),
            &["a"],
            &[
                ("pubspec.yaml", SCAFFOLD_PUBSPEC),
                (".dart_tool/package_config.json", SCAFFOLD_CONFIG),
            ],
        );
        let driver =
            DartqDriver::from_program("sh".to_string(), vec!["-c".into(), "exit 64".into()]);
        let err = dartq_phase(&[(None, corpus_dir)], &driver, OutputFormat::Machine)
            .expect_err("exit 64 must hard-fail");
        let DartqPhaseError::Hard(msg) = err else {
            panic!("expected Hard, got {err:?}");
        };
        assert!(msg.contains("Some(64)"), "historical error text: {msg}");
        assert!(msg.contains("valid: 0-3"), "names the valid set: {msg}");

        fs::remove_dir_all(&root).ok();
    }

    /// Subset farms: `.dart` payload as symlinks into the real corpus,
    /// scaffolding copied, layout mirrored, unselected files absent.
    #[test]
    fn subset_farm_symlinks_payload_and_copies_scaffolding() {
        let root = std::env::temp_dir().join(format!("dq_bisect_farm_{}", std::process::id()));
        fs::create_dir_all(&root).expect("mkdir");
        let corpus_dir = write_corpus(
            &root.join("corpus"),
            &["top", "inner"],
            &[
                ("pubspec.yaml", SCAFFOLD_PUBSPEC),
                (".dart_tool/package_config.json", SCAFFOLD_CONFIG),
                ("notes.txt", "not a candidate\n"),
            ],
        );
        let dest = root.join("farm");
        build_subset(&corpus_dir, &[PathBuf::from("lib/inner.dart")], &dest).expect("farm");

        // Payload is a symlink resolving to the real file; unselected files absent.
        let linked = dest.join("lib/inner.dart");
        let meta = linked.symlink_metadata().expect("link exists");
        assert!(meta.file_type().is_symlink(), "payload rides a symlink");
        assert_eq!(
            fs::read_link(&linked).expect("link target"),
            corpus_dir.join("lib/inner.dart")
        );
        assert!(!dest.join("lib/top.dart").exists());
        assert!(
            !dest.join("notes.txt").exists(),
            "non-candidates never enter farms"
        );
        // Scaffolding copied as regular files with identical content.
        let pubspec = dest.join("pubspec.yaml");
        assert!(pubspec.symlink_metadata().expect("pubspec").is_file());
        assert_eq!(fs::read_to_string(pubspec).expect("read"), SCAFFOLD_PUBSPEC);
        assert_eq!(
            fs::read_to_string(dest.join(".dart_tool/package_config.json")).expect("read"),
            SCAFFOLD_CONFIG
        );

        fs::remove_dir_all(&root).ok();
    }

    /// Budget formula: `2 * ceil(log2(N)) + 4`.
    #[test]
    fn budget_formula() {
        assert_eq!(ceil_log2(1), 0);
        assert_eq!(ceil_log2(2), 1);
        assert_eq!(ceil_log2(4), 2);
        assert_eq!(ceil_log2(5), 3);
        assert_eq!(ceil_log2(16), 4);
        assert_eq!(ceil_log2(279), 9);
    }
}
