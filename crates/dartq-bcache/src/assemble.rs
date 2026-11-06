//! The `flutter assemble` argv model: the build-dir name template of one
//! assemble invocation, so the outer shim's post-run ingest can replay
//! the dir under a recomputed name in any tree.
//!
//! The platform build systems behind `flutter build|run` (Gradle, Xcode,
//! CMake) invoke `flutter assemble …` as a child process with every
//! `-dKEY=VALUE` define and the `--output` dir in argv — through the
//! SDK's absolute `$FLUTTER_ROOT/bin/flutter` path, which no PATH shim
//! can intercept. The outer shim's process-table observer
//! ([`crate::observer`]) therefore reads each such invocation's argv
//! while the build runs and hands it to [`capture_templates`], which
//! rebuilds the exact `Environment.defines` map and hashed output-dir
//! string the flutter_tools 3.47.1 `AssembleCommand` assembles and
//! computes the `.dart_tool/flutter_build/<name>` dir name that
//! invocation will use, as one [`AssembleCapture`].
//!
//! A template is trusted only after the outer POST verifies its `name`
//! against a dir that actually materialized, so a miscomputed name — or
//! an argv misread off the process table — is dropped, never ingested.
//! That check is also the arbiter of the one unavoidable misread: a
//! `ps`-sourced argv is whitespace-split, so a define VALUE with
//! embedded spaces arrives truncated with its remainder as stray
//! positional tokens — the DEFAULT multi-arch apk build's
//! `-dAndroidArchs=android-arm android-arm64 android-x64`. The capture
//! enumerates the join-variants of every such orphan run
//! ([`capture_templates`]) and the POST accepts exactly the variant
//! whose dir materialized, so the split can cost a capture only where
//! even the joined variants miss, never misplace one.
//!
//! Everything here is pure except the pubspec read of the
//! deferred-components heuristic.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use base64::Engine as _;

use crate::args_model::{ArgModelError, flutter_build_dir_name};

/// One recorded assemble invocation: everything needed to recompute its
/// hash-dir name in another tree.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct AssembleCapture {
    /// The `Environment.defines` map exactly as the invocation hashed it.
    pub defines: BTreeMap<String, String>,
    /// The output-dir string as hashed at capture time (absolute; a
    /// relative `--output` is joined against the capture cwd, as
    /// flutter_tools joins it against the project directory).
    pub output_dir: String,
    /// `flutter_build_dir_name(engine_revision, defines, output_dir)` at
    /// capture time — verified by the outer POST against the dir that
    /// actually appeared.
    pub name: String,
}

/// Where one argv option's value lands.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Slot {
    /// `-d/--define`: a `KEY=VALUE` pair (flutter_tools 3.47.1).
    Define,
    /// `--dart-define`/`-D`/`--DartDefines`: one padded-standard-base64
    /// `k=v` pair.
    DartDefine,
    /// `--dart-define-from-file`: unrecorded (see [`capture_templates`]).
    DefineFile,
    /// `-o/--output`: the output directory.
    Output,
    /// `--ExtraFrontEndOptions`: a multi option.
    ExtraFrontEnd,
    /// `--ExtraGenSnapshotOptions`: a multi option.
    ExtraGenSnapshot,
    /// `--local-engine` and kin: selects local artifacts, which makes the
    /// hashed engine revision null.
    LocalEngine,
    /// A value option that feeds no define (`--depfile`,
    /// `--build-inputs`, `--build-outputs`, `--input`,
    /// `--performance-measurement-file`, `--resource-pool-size`,
    /// `--device-id`).
    IgnoredValue,
    /// A flag that consumes no value (`--verbose`, `--quiet`,
    /// `--no-version-check`, …).
    Flag,
}

/// The options a pre-`assemble` global segment can carry.
fn global_slot(name: &str) -> Option<Slot> {
    match name {
        "local-engine" | "local-engine-src-path" | "local-engine-host" | "local-web-sdk" => {
            Some(Slot::LocalEngine)
        }
        "device-id" => Some(Slot::IgnoredValue),
        "verbose" | "quiet" | "version-check" | "no-version-check" | "suppress-analytics"
        | "help" | "prefixed-error" | "ci" | "color" | "no-color" | "wrap" | "no-wrap" => {
            Some(Slot::Flag)
        }
        _ => None,
    }
}

/// The options the `assemble` command declares (flutter_tools 3.47.1
/// `AssembleCommand` + the helpers it calls), plus the globals that also
/// resolve after the command through `package:args`' parent fallback.
fn command_slot(name: &str) -> Option<Slot> {
    match name {
        "define" => Some(Slot::Define),
        // `DartDefines` is the historical alias of `dart-define`
        // (`usesDartDefineOption`), and its value is a comma-joined
        // base64 list exactly as gradle passes it.
        "dart-define" | "DartDefines" => Some(Slot::DartDefine),
        "dart-define-from-file" => Some(Slot::DefineFile),
        "input"
        | "depfile"
        | "build-inputs"
        | "build-outputs"
        | "performance-measurement-file"
        | "resource-pool-size" => Some(Slot::IgnoredValue),
        "output" => Some(Slot::Output),
        "ExtraFrontEndOptions" | "extra-front-end-options" => Some(Slot::ExtraFrontEnd),
        "ExtraGenSnapshotOptions" | "extra-gen-snapshot-options" => Some(Slot::ExtraGenSnapshot),
        _ => global_slot(name),
    }
}

fn short_slot(letter: u8) -> Option<Slot> {
    match letter {
        b'd' => Some(Slot::Define),
        b'D' => Some(Slot::DartDefine),
        b'i' => Some(Slot::IgnoredValue),
        b'o' => Some(Slot::Output),
        b'v' => Some(Slot::Flag),
        _ => None,
    }
}

/// A run of positional tokens directly following a define value: the ps
/// whitespace split of a spaced define value (`-dK=a b c` arrives as
/// `-dK=a` plus the orphans `b` `c`). [`capture_templates`] folds each
/// prefix of the run back into the value as its own candidate parse.
#[derive(Clone)]
struct OrphanRun {
    /// Index into `AssembleArgs::defines_raw`: the truncated define.
    define: usize,
    /// Index into `AssembleArgs::targets` of the run's first orphan.
    target: usize,
    /// The run's tokens, in argv order.
    tokens: Vec<String>,
}

/// Everything one assemble argv contributed.
#[derive(Default, Clone)]
struct AssembleArgs {
    defines_raw: Vec<String>,
    dart_defines_raw: Vec<String>,
    output: Option<String>,
    front_end: Vec<String>,
    gen_snapshot: Vec<String>,
    local_engine: bool,
    targets: Vec<String>,
    /// The define value the NEXT positional would continue, if any
    /// (orphan-run detection; see [`OrphanRun`]).
    pending_define: Option<usize>,
    /// An orphan run is still extending through consecutive positionals.
    orphan_run_open: bool,
    orphan_runs: Vec<OrphanRun>,
}

impl AssembleArgs {
    /// Applies one option value to its slot.
    fn record(&mut self, slot: Slot, value: &str) -> Result<(), ArgModelError> {
        match slot {
            Slot::Define => self.defines_raw.push(value.to_owned()),
            Slot::DartDefine => self.dart_defines_raw.push(value.to_owned()),
            Slot::DefineFile => {
                return Err(ArgModelError::Unsupported(
                    "`flutter assemble --dart-define-from-file` folds file contents into defines this crate does not model".into(),
                ));
            }
            Slot::Output => self.output = Some(value.to_owned()),
            Slot::ExtraFrontEnd => self.front_end.push(value.to_owned()),
            Slot::ExtraGenSnapshot => self.gen_snapshot.push(value.to_owned()),
            Slot::LocalEngine => self.local_engine = true,
            Slot::IgnoredValue | Slot::Flag => {}
        }
        Ok(())
    }

    /// [`Self::record`], tagging a define value so a positional directly
    /// after it opens an [`OrphanRun`].
    fn record_tagged(&mut self, slot: Slot, value: &str) -> Result<(), ArgModelError> {
        self.record(slot, value)?;
        if slot == Slot::Define {
            self.pending_define = Some(self.defines_raw.len() - 1);
        }
        Ok(())
    }

    /// Ends any define/orphan adjacency: every non-positional token
    /// breaks the run (only consecutive positionals continue one).
    fn close_orphan_run(&mut self) {
        self.pending_define = None;
        self.orphan_run_open = false;
    }

    /// Walks one argv segment with `slot_of` (long names) and
    /// `shorts_of` (single letters), classifying tokens the way
    /// `package:args` does: `--` ends options; `--name=value` carries an
    /// inline value; a value option without one consumes the next token,
    /// whatever it starts with; `-x` is short, `-xyz` attaches `yz` to
    /// `-x`; every other token is positional (an assemble target). An
    /// unknown option is assumed value-taking — conservative skipping
    /// only loses a capture, never misparses a known one. Positional
    /// tokens are recorded as targets only in the COMMAND segment
    /// (`count_targets`): flutter's target/debug checks iterate the
    /// command's `argResults.rest` alone, and a stray token before the
    /// command is neither a target nor a veto.
    fn walk(
        &mut self,
        tokens: &[String],
        slot_of: fn(&str) -> Option<Slot>,
        shorts_of: fn(u8) -> Option<Slot>,
        count_targets: bool,
    ) -> Result<(), ArgModelError> {
        let mut i = 0;
        let mut options = true;
        while i < tokens.len() {
            let token = tokens[i].as_str();
            if options && token == "--" {
                options = false;
                self.close_orphan_run();
                i += 1;
                continue;
            }
            if let Some(body) = token.strip_prefix("--").filter(|_| options) {
                self.close_orphan_run();
                let (name, inline) = match body.split_once('=') {
                    Some((name, value)) => (name, Some(value)),
                    None => (body, None),
                };
                match slot_of(name) {
                    Some(Slot::Flag) => {}
                    Some(slot) => match inline {
                        Some(value) => self.record_tagged(slot, value)?,
                        None => {
                            let value = tokens.get(i + 1).ok_or_else(|| {
                                ArgModelError::Invalid(format!(
                                    "option `{token}` is missing its value"
                                ))
                            })?;
                            self.record_tagged(slot, value)?;
                            i += 1;
                        }
                    },
                    None => {
                        if inline.is_none() {
                            i += 1; // an unknown option may consume a value
                        }
                    }
                }
            } else if options
                && token.len() >= 2
                && token.as_bytes()[0] == b'-'
                && token.as_bytes()[1].is_ascii_alphanumeric()
            {
                self.close_orphan_run();
                match shorts_of(token.as_bytes()[1]) {
                    Some(slot) if token.len() == 2 => {
                        let value = tokens.get(i + 1).ok_or_else(|| {
                            ArgModelError::Invalid(format!("option `{token}` is missing its value"))
                        })?;
                        self.record_tagged(slot, value)?;
                        i += 1;
                    }
                    Some(slot) => self.record_tagged(slot, &token[2..])?,
                    None if token.len() == 2 => {
                        i += 1; // an unknown short may consume a value
                    }
                    None => {} // a cluster of unknown short flags
                }
            } else if count_targets {
                self.targets.push(token.to_owned());
                if let Some(define) = self.pending_define.take() {
                    // The whitespace split of a spaced define value: the
                    // first orphan right after the value opens a run.
                    self.orphan_runs.push(OrphanRun {
                        define,
                        target: self.targets.len() - 1,
                        tokens: vec![token.to_owned()],
                    });
                    self.orphan_run_open = true;
                } else if self.orphan_run_open {
                    if let Some(run) = self.orphan_runs.last_mut() {
                        run.tokens.push(token.to_owned());
                    }
                }
            }
            i += 1;
        }
        Ok(())
    }
}

/// Whether `argv` is a `flutter assemble` invocation: a bare `assemble`
/// token exists and no earlier token can be consuming it as a value.
#[must_use]
pub fn is_assemble_argv(argv: &[String]) -> bool {
    locate_command(argv).is_some()
}

/// The index of the `assemble` command token, walking pre-command tokens
/// as `package:args` would: inline values are self-contained, a known
/// flag consumes nothing, and anything else option-like is assumed to
/// consume the next token (a skipped command token only loses a capture).
fn locate_command(argv: &[String]) -> Option<usize> {
    let mut i = 0;
    while i < argv.len() {
        let token = argv[i].as_str();
        if token == "assemble" {
            return Some(i);
        }
        let consumes_next = if let Some(body) = token.strip_prefix("--") {
            !body.contains('=') && global_slot(body).is_none()
        } else {
            // `-d <device>` / `-P <arg>` before the command.
            matches!(token, "-d" | "-P")
        };
        i += usize::from(consumes_next) + 1;
    }
    None
}

/// Upper bound on enumerated join-variants per argv: the real producers
/// yield at most a dozen (a spaced archs define plus the trailing-target
/// run); an argv whose positional/define adjacency explodes past this is
/// not a shape flutter produces, and the as-tokenized parse alone —
/// accepted or dropped by the POST name check — is the safe answer.
const MAX_JOIN_VARIANTS: usize = 64;

/// Rebuilds every [`AssembleCapture`] one `flutter assemble` argv can
/// yield. The first is the argv as tokenized; each positional run
/// directly following a define value additionally yields one capture
/// per prefix of the run folded back into the value — the split is
/// unrecoverable from the tokens alone, so every fold stays a candidate
/// and the outer POST's name verification accepts exactly the variant
/// whose dir materialized. That enumeration answers the ps whitespace
/// split of a spaced value (e.g. the multi-arch `-dAndroidArchs=…
/// android-arm android-arm64 android-x64`), so `enumerate_joins`
/// carries the argv's source: `true` for a whitespace-split line, and
/// `false` for a kernel-read argv whose tokens are exact and yield the
/// as-tokenized capture alone. Every capture carries the define map
/// exactly as the flutter_tools 3.47.1 `AssembleCommand` assembles it
/// for the `Environment` factory —
///
/// * every `-dK=V` value, then every `--dart-define` value
///   (padded-standard-base64 `k=v`, decoded to its own top-level key,
///   overriding a `-d` key of the same name);
/// * `ExtraGenSnapshotOptions`/`ExtraFrontEndOptions` (the values joined
///   with `,`) when the option was passed at all;
/// * `DartDefines`: the raw base64 values joined with `,` when any are
///   non-empty;
/// * `DeferredComponents`: `true` only for a deferred-components target
///   set on a non-debug invocation whose pubspec declares deferred
///   components (a line-level heuristic — a wrong guess shifts the name,
///   and the outer POST drops the template), else `false`;
///
/// and the output-dir string hashed as given: absolute `--output`
/// verbatim, a relative one joined against `cwd` (flutter joins it
/// against the project directory, which is its working directory). A
/// local-engine invocation hashes a null engine revision, so
/// `toolchain_engine_revision` is used only without `--local-engine*`.
///
/// # Errors
///
/// [`ArgModelError::Invalid`] for an argv flutter itself rejects (a
/// missing `--output`, a define without `=`, a missing option value,
/// non-base64 or non-UTF-8 dart defines); [`ArgModelError::Unsupported`]
/// for `--dart-define-from-file` (its file-folded defines are unmodeled)
/// and an argv with no `assemble` token. The conditions are checked on
/// the as-tokenized parse; no fold can introduce an error it did not
/// have.
pub fn capture_templates(
    argv: &[String],
    cwd: &Path,
    toolchain_engine_revision: &str,
    enumerate_joins: bool,
) -> Result<Vec<AssembleCapture>, ArgModelError> {
    let at = locate_command(argv)
        .ok_or_else(|| ArgModelError::Unsupported("no `assemble` command token in argv".into()))?;
    let mut args = AssembleArgs::default();
    args.walk(&argv[..at], global_slot, global_short, false)?;
    args.walk(&argv[at + 1..], command_slot, short_slot, true)?;

    let mut captures = vec![finish_args(&args, cwd, toolchain_engine_revision)?];
    let runs = std::mem::take(&mut args.orphan_runs);
    let total: usize = runs.iter().map(|run| run.tokens.len() + 1).product();
    if enumerate_joins && total <= MAX_JOIN_VARIANTS {
        // Combination 0 — every fold zero — is the as-tokenized capture
        // already in place; the rest are mixed-radix digits of the runs.
        for combination in 1..total {
            let mut rest = combination;
            let folds: Vec<usize> = runs
                .iter()
                .map(|run| {
                    let radix = run.tokens.len() + 1;
                    let fold = rest % radix;
                    rest /= radix;
                    fold
                })
                .collect();
            captures.push(fold_variant(
                &args,
                &runs,
                &folds,
                cwd,
                toolchain_engine_revision,
            )?);
        }
    }
    Ok(captures)
}

/// The captured template of one parsed argv (see [`capture_templates`]
/// for the define-map contract).
fn finish_args(
    args: &AssembleArgs,
    cwd: &Path,
    toolchain_engine_revision: &str,
) -> Result<AssembleCapture, ArgModelError> {
    let engine_revision = if args.local_engine {
        ""
    } else {
        toolchain_engine_revision
    };
    let decoded = decode_dart_defines(&args.dart_defines_raw)?;

    let mut defines: BTreeMap<String, String> = BTreeMap::new();
    for chunk in args.defines_raw.iter().chain(decoded.iter()) {
        if chunk.is_empty() {
            continue;
        }
        let Some((key, value)) = chunk.split_once('=') else {
            return Err(ArgModelError::Invalid(format!(
                "improperly formatted define flag: {chunk}"
            )));
        };
        defines.insert(key.to_owned(), value.to_owned());
    }
    if !args.gen_snapshot.is_empty() {
        defines.insert(
            "ExtraGenSnapshotOptions".to_owned(),
            args.gen_snapshot.join(","),
        );
    }
    let raw_defines: Vec<&str> = args
        .dart_defines_raw
        .iter()
        .map(String::as_str)
        .filter(|value| !value.is_empty())
        .collect();
    if !raw_defines.is_empty() {
        defines.insert("DartDefines".to_owned(), raw_defines.join(","));
    }
    defines.insert(
        "DeferredComponents".to_owned(),
        deferred_components(&args.targets, cwd).to_string(),
    );
    if !args.front_end.is_empty() {
        defines.insert("ExtraFrontEndOptions".to_owned(), args.front_end.join(","));
    }

    let output = args
        .output
        .clone()
        .ok_or_else(|| ArgModelError::Invalid("`flutter assemble` requires --output".into()))?;
    let output_dir = if Path::new(&output).is_absolute() {
        output
    } else {
        cwd.join(&output).to_string_lossy().into_owned()
    };
    let name = flutter_build_dir_name(engine_revision, &defines, Path::new(&output_dir));
    Ok(AssembleCapture {
        defines,
        output_dir,
        name,
    })
}

/// One join-variant: `folds[i]` orphans of run i folded back into its
/// define value (single-space joined), the rest staying positional.
/// Later runs are folded first so earlier runs' target indices stay
/// valid through the drains.
fn fold_variant(
    base: &AssembleArgs,
    runs: &[OrphanRun],
    folds: &[usize],
    cwd: &Path,
    toolchain_engine_revision: &str,
) -> Result<AssembleCapture, ArgModelError> {
    let mut args = base.clone();
    for (run, &fold) in runs.iter().zip(folds).rev() {
        if fold == 0 {
            continue;
        }
        if let Some(value) = args.defines_raw.get_mut(run.define) {
            for token in &run.tokens[..fold] {
                value.push(' ');
                value.push_str(token);
            }
        }
        args.targets.drain(run.target..run.target + fold);
    }
    finish_args(&args, cwd, toolchain_engine_revision)
}

/// Decodes every `--dart-define` value the way flutter does
/// (`decodeDartDefines`): each value is a comma-joined list of
/// padded-standard-base64 `k=v` pairs (gradle passes the whole joined
/// list in one `--DartDefines` token), each part strictly UTF-8-decoded;
/// empty parts decode to nothing.
fn decode_dart_defines(raw: &[String]) -> Result<Vec<String>, ArgModelError> {
    let mut decoded = Vec::new();
    for value in raw {
        for part in value.split(',') {
            if part.is_empty() {
                continue;
            }
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(part)
                .map_err(|_| {
                    ArgModelError::Invalid(format!("dart define is not base64 encoded: {part}"))
                })?;
            decoded.push(String::from_utf8(bytes).map_err(|_| {
                ArgModelError::Invalid(format!("dart define does not decode to UTF-8: {part}"))
            })?);
        }
    }
    Ok(decoded)
}

/// The assemble gate for `DeferredComponents` (flutter_tools
/// `assemble.dart`): one of the six deferred-components bundle targets
/// (`android.dart`'s `deferredComponentsTargets`), no target name
/// containing `debug`, and a pubspec that declares the section.
fn deferred_components(targets: &[String], cwd: &Path) -> bool {
    const DEFERRED_TARGETS: [&str; 6] = [
        "android_aot_deferred_components_bundle_profile_android-arm",
        "android_aot_deferred_components_bundle_profile_android-arm64",
        "android_aot_deferred_components_bundle_profile_android-x64",
        "android_aot_deferred_components_bundle_release_android-arm",
        "android_aot_deferred_components_bundle_release_android-arm64",
        "android_aot_deferred_components_bundle_release_android-x64",
    ];
    targets
        .iter()
        .any(|target| DEFERRED_TARGETS.contains(&target.as_str()))
        && !targets.iter().any(|target| target.contains("debug"))
        && pubspec_declares_deferred_components(cwd)
}

/// Whether the pubspec at `cwd` declares `flutter: deferred-components:`.
/// Line-level: true only for an indented `deferred-components:` KEY
/// inside the top-level `flutter:` section — first on the line, with or
/// without an inline value or trailing comment (`[]`, `# note`, a bare
/// colon all declare, mirroring the manifest parser: any presence of
/// the key counts; a commented-out line does not). Unreadable or absent
/// pubspec reads as false.
fn pubspec_declares_deferred_components(cwd: &Path) -> bool {
    let Ok(text) = fs::read_to_string(cwd.join("pubspec.yaml")) else {
        return false;
    };
    let mut in_flutter = false;
    for line in text.lines() {
        if line.starts_with("flutter:") {
            in_flutter = true;
            continue;
        }
        if !line.starts_with([' ', '\t']) && !line.trim().is_empty() {
            in_flutter = false;
        }
        if in_flutter && line.trim().starts_with("deferred-components:") {
            return true;
        }
    }
    false
}

/// The global short options before the command: `-d` selects a device,
/// `-P` passes a project arg; both consume a value and feed no define.
fn global_short(letter: u8) -> Option<Slot> {
    match letter {
        b'd' | b'P' => Some(Slot::IgnoredValue),
        b'v' => Some(Slot::Flag),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(tokens: &[&str]) -> Vec<String> {
        tokens.iter().map(|token| (*token).to_owned()).collect()
    }

    fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    fn capture(tokens: &[&str]) -> AssembleCapture {
        let tmp = tempfile::tempdir().expect("tempdir");
        capture_argv(&strings(tokens), tmp.path(), "enginerev0000000000000000")
    }

    /// The as-tokenized parse of one argv: the first of the enumerated
    /// join-variants.
    fn capture_argv(argv: &[String], cwd: &Path, engine_revision: &str) -> AssembleCapture {
        let mut all = capture_templates(argv, cwd, engine_revision, true)
            .unwrap_or_else(|err| panic!("{argv:?}: {err}"));
        assert!(!all.is_empty(), "an Ok capture always has the base parse");
        all.remove(0)
    }

    #[test]
    fn detects_the_assemble_command_token() {
        // Real producer shapes: gradle (`--quiet assemble …`), Xcode
        // (`$FLUTTER_ROOT/bin/flutter assemble --no-version-check …`),
        // tool_backend. And the shapes that are NOT assemble.
        assert!(is_assemble_argv(&strings(&[
            "assemble",
            "-dBuildMode=release"
        ])));
        assert!(is_assemble_argv(&strings(&[
            "--quiet",
            "assemble",
            "--no-version-check",
            "--output",
            "/out"
        ])));
        assert!(is_assemble_argv(&strings(&[
            "--local-engine",
            "android_release",
            "assemble",
            "-o",
            "/out"
        ])));
        assert!(is_assemble_argv(&strings(&[
            "assemble",
            "android_aot_bundle_release_arm64"
        ])));
        assert!(!is_assemble_argv(&strings(&["build", "apk"])));
        assert!(!is_assemble_argv(&strings(&["run"])));
        assert!(!is_assemble_argv(&strings(&[])));
        // An option value that happens to spell `assemble` is consumed.
        assert!(!is_assemble_argv(&strings(&["--frob", "assemble"])));
    }

    #[test]
    fn captures_the_gradle_shape_define_map() {
        // The argv gradle's BaseFlutterTaskHelper builds for a release
        // APK, with dart defines base64-encoded (`API=prod`).
        let got = capture(&[
            "--quiet",
            "assemble",
            "--no-version-check",
            "--depfile",
            "/work/app/build/app/intermediates/flutter/paidRelease/flutter_build.d",
            "--output",
            "/work/app/build/app/intermediates/flutter/paidRelease",
            "-dTargetFile=lib/main.dart",
            "-dTargetPlatform=android",
            "-dBuildMode=release",
            "-dTrackWidgetCreation=true",
            "--DartDefines=QVBJPXByb2Q=",
            "-dFlavor=paid",
            "--ExtraGenSnapshotOptions=",
            "-dAndroidArchs=android-arm64",
            "-dMinSdkVersion=21",
            "android_aot_bundle_release_android-arm64",
        ]);
        assert_eq!(
            got.defines,
            map(&[
                ("AndroidArchs", "android-arm64"),
                ("API", "prod"),
                ("BuildMode", "release"),
                ("DartDefines", "QVBJPXByb2Q="),
                ("DeferredComponents", "false"),
                ("ExtraGenSnapshotOptions", ""),
                ("Flavor", "paid"),
                ("MinSdkVersion", "21"),
                ("TargetFile", "lib/main.dart"),
                ("TargetPlatform", "android"),
                ("TrackWidgetCreation", "true"),
            ])
        );
        assert_eq!(
            got.output_dir,
            "/work/app/build/app/intermediates/flutter/paidRelease"
        );
        assert_eq!(
            got.name,
            flutter_build_dir_name(
                "enginerev0000000000000000",
                &got.defines,
                Path::new(&got.output_dir)
            )
        );
    }

    #[test]
    fn captures_the_xcode_shape_and_empty_valued_options() {
        let got = capture(&[
            "--no-version-check",
            "assemble",
            "--output=/out/Build/Products/Release-iphoneos/",
            "-dTargetPlatform=ios",
            "-dTargetFile=../../.dart_tool/flutter_build/x",
            "-dBuildMode=release",
            "-dDartObfuscation=false",
            "-dIosArchs=arm64",
            "--ExtraGenSnapshotOptions=",
            "--DartDefines=",
            "--ExtraFrontEndOptions=",
            "release_ios_bundle_flutter_assets",
        ]);
        // Empty-valued multi options ARE define keys when passed
        // (flutter's `wasParsed`), except DartDefines, which an empty
        // list never sets. The trailing slash of `--output` is hashed
        // verbatim.
        assert_eq!(
            got.defines,
            map(&[
                ("BuildMode", "release"),
                ("DartObfuscation", "false"),
                ("DeferredComponents", "false"),
                ("ExtraFrontEndOptions", ""),
                ("ExtraGenSnapshotOptions", ""),
                ("IosArchs", "arm64"),
                ("TargetFile", "../../.dart_tool/flutter_build/x"),
                ("TargetPlatform", "ios"),
            ])
        );
        assert_eq!(got.output_dir, "/out/Build/Products/Release-iphoneos/");
    }

    #[test]
    fn dart_defines_decode_and_override_d_defines() {
        // `K=V` as padded standard base64: `QVBJPXByb2Q=` is `API=prod`,
        // `RkxVVFRFUl9BUFBfRkxBVk9SPXBhaWQ=` is `FLUTTER_APP_FLAVOR=paid`.
        let got = capture(&[
            "assemble",
            "-o",
            "/out",
            "-dAPI=stale",
            "--DartDefines=RkxVVFRFUl9BUFBfRkxBVk9SPXBhaWQ=,QVBJPXByb2Q=",
        ]);
        assert_eq!(got.defines.get("API").map(String::as_str), Some("prod"));
        assert_eq!(
            got.defines.get("FLUTTER_APP_FLAVOR").map(String::as_str),
            Some("paid")
        );
        assert_eq!(
            got.defines.get("DartDefines").map(String::as_str),
            Some("RkxVVFRFUl9BUFBfRkxBVk9SPXBhaWQ=,QVBJPXByb2Q=")
        );
        // A separate `--dart-define` spelling lands the same way.
        let separated = capture(&["assemble", "-o", "/out", "--dart-define", "QVBJPXByb2Q="]);
        assert_eq!(
            separated.defines.get("API").map(String::as_str),
            Some("prod")
        );
        assert_eq!(
            separated.defines.get("DartDefines").map(String::as_str),
            Some("QVBJPXByb2Q=")
        );
    }

    /// A gradle-nested assemble argv as a macOS `ps` poll delivers it
    /// (verbatim token shapes and order, whitespace-split): the
    /// multi-arch `-dAndroidArchs` value arrives truncated with its
    /// remainder as positional orphans. Captured from a real build,
    /// then path-fictionalized (SDK root, project root) with every
    /// token shape preserved — the paths are fixture inputs, not
    /// references into any checkout-local tree. The pinned names are
    /// precomputed offline with the flutter_tools 3.47.1 dir-name hash
    /// (exactly the crate's `flutter_build_dir_name`) over THIS argv,
    /// so the goldens stay non-circular: the full-value join is the
    /// name the real build materialized (its dir-name hash is how the
    /// fixture was first validated), and the as-split parse (whose
    /// name never materialized — the e2e red this regresses) is the
    /// base.
    #[test]
    fn the_space_split_gradle_argv_reaches_the_observed_dir_name() {
        let tmp = tempfile::tempdir().expect("tempdir");
        const ENGINE: &str = "5d531788691ec3404cac0cee66ead4007b177363";
        let output = "/work/myapp/build/app/intermediates/flutter/debug";
        let argv = strings(&[
            "/opt/flutter471/bin/cache/dart-sdk/bin/dartvm",
            "--resolved_executable_name=/opt/flutter471/bin/cache/dart-sdk/bin/dart",
            "--executable_name=/opt/flutter471/bin/cache/dart-sdk/bin/dart",
            "--packages=/opt/flutter471/packages/flutter_tools/.dart_tool/package_config.json",
            "/opt/flutter471/bin/cache/flutter_tools.snapshot",
            "--quiet",
            "assemble",
            "--no-version-check",
            "--depfile",
            "/work/myapp/build/app/intermediates/flutter/debug/flutter_build.d",
            "--output",
            output,
            "-dTargetFile=lib/main.dart",
            "-dTargetPlatform=android",
            "-dBuildMode=debug",
            "-dTrackWidgetCreation=true",
            "--DartDefines=RkxVVFRFUl9CVUlMRF9OQU1FPTIuMy42,RkxVVFRFUl9CVUlMRF9OVU1CRVI9OTk=,RkxVVFRFUl9WRVJTSU9OPTMuNDcuMQ==,RkxVVFRFUl9DSEFOTkVMPXN0YWJsZQ==,RkxVVFRFUl9HSVRfVVJMPWh0dHBzOi8vZ2l0aHViLmNvbS9mbHV0dGVyL2ZsdXR0ZXIuZ2l0,RkxVVFRFUl9GUkFNRVdPUktfUkVWSVNJT049NjY1NTQ4MmVjMA==,RkxVVFRFUl9FTkdJTkVfUkVWSVNJT049NWQ1MzE3ODg2OQ==,RkxVVFRFUl9EQVJUX1ZFUlNJT049My4xMy4x",
            "-dFlavor=",
            "-dAndroidArchs=android-arm",
            "android-arm64",
            "android-x64",
            "-dMinSdkVersion=24",
            "debug_android_application",
        ]);
        let variants =
            capture_templates(&argv, tmp.path(), ENGINE, true).expect("the real argv must parse");
        // Two orphan runs — the split archs value (3 folds) and the
        // trailing target after the last define (2 folds) — plus the
        // as-tokenized base: 3 * 2 captures.
        assert_eq!(variants.len(), 6);
        assert_eq!(
            variants[0].defines.get("AndroidArchs").map(String::as_str),
            Some("android-arm")
        );
        assert_eq!(variants[0].name, "50aa28fa2bca19ee5f692df6c44725dd");

        let full = variants
            .iter()
            .find(|capture| {
                capture.defines.get("AndroidArchs").map(String::as_str)
                    == Some("android-arm android-arm64 android-x64")
            })
            .expect("the full-value join variant");
        assert_eq!(full.name, "faea472ffd84b691840e8506221fea8e");
        assert_eq!(full.output_dir, output);
        assert_eq!(
            full.defines.get("MinSdkVersion").map(String::as_str),
            Some("24")
        );
        assert_eq!(
            full.defines
                .get("FLUTTER_ENGINE_REVISION")
                .map(String::as_str),
            Some("5d53178869")
        );

        let mut names: Vec<&str> = variants
            .iter()
            .map(|capture| capture.name.as_str())
            .collect();
        names.sort_unstable();
        assert!(
            names.windows(2).all(|pair| pair[0] != pair[1]),
            "every fold is a distinct candidate"
        );
    }

    /// `-dK=a b c` as ps splits it: the as-tokenized parse plus one
    /// capture per folded prefix of the orphan run.
    #[test]
    fn join_variants_fold_each_prefix_of_a_spaced_define_value() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let argv = strings(&["assemble", "-o", "/out", "-dK=a", "b", "c"]);
        let variants = capture_templates(&argv, tmp.path(), "e", true).expect("parse");
        assert_eq!(variants.len(), 3);
        let values: Vec<&str> = variants
            .iter()
            .map(|capture| {
                capture
                    .defines
                    .get("K")
                    .map(String::as_str)
                    .expect("the K define")
            })
            .collect();
        assert_eq!(values, ["a", "a b", "a b c"]);
    }

    /// The real tail shape: the archs run closes at the next option, and
    /// the assemble target after the LAST define opens a run of its own —
    /// folding the target is enumerated (and loses on the POST name
    /// check) rather than misparsed.
    #[test]
    fn a_target_after_the_last_define_is_its_own_orphan_run() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let argv = strings(&["assemble", "-o", "/out", "-dA=x", "y", "z", "-dB=1", "t"]);
        let variants = capture_templates(&argv, tmp.path(), "e", true).expect("parse");
        assert_eq!(variants.len(), 6, "3 A-folds x 2 B-folds");
        let true_parse = variants
            .iter()
            .find(|capture| {
                capture.defines.get("A").map(String::as_str) == Some("x y z")
                    && capture.defines.get("B").map(String::as_str) == Some("1")
            })
            .expect("the true parse is a candidate");
        assert!(
            variants
                .iter()
                .any(|capture| capture.defines.get("B").map(String::as_str) == Some("1 t"))
        );
        assert_ne!(true_parse.name, variants[0].name);
    }

    /// No positional directly after any define value: exactly one
    /// capture, the pre-variant behavior.
    #[test]
    fn an_argv_without_orphans_yields_the_single_parse() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let argv = strings(&["assemble", "-o", "/out", "t", "-dA=1"]);
        let variants = capture_templates(&argv, tmp.path(), "e", true).expect("parse");
        assert_eq!(variants.len(), 1);
    }

    /// Define/positional adjacency beyond the variant budget is not a
    /// shape flutter produces: the as-tokenized parse alone, accepted or
    /// dropped by the POST name check, is the safe answer.
    #[test]
    fn an_exploding_adjacency_degrades_to_the_base_parse() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut tokens = vec!["assemble".to_owned(), "-o".to_owned(), "/out".to_owned()];
        for index in 0..7 {
            tokens.push(format!("-dK{index}=v"));
            tokens.push(format!("o{index}"));
        }
        let variants = capture_templates(&tokens, tmp.path(), "e", true).expect("parse");
        assert_eq!(variants.len(), 1, "2^7 folds exceed the budget");
    }

    /// A kernel-read argv (true NUL-separated elements) is exact: the
    /// multi-arch define value is one token and the trailing target is a
    /// real positional — no fold is enumerated, the as-tokenized capture
    /// is the answer the observer takes.
    #[test]
    fn kernel_argv_skips_the_join_variant_enumeration() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let argv = strings(&[
            "assemble",
            "-o",
            "/out/release",
            "-dAndroidArchs=android-arm android-arm64 android-x64",
            "android_aot_bundle_release",
        ]);
        let captures = capture_templates(&argv, tmp.path(), "e", false).expect("parse");
        assert_eq!(captures.len(), 1, "the exact parse alone");
        assert_eq!(
            captures[0].defines.get("AndroidArchs").map(String::as_str),
            Some("android-arm android-arm64 android-x64"),
            "the spaced value is captured whole, the target unfolded"
        );
    }

    #[test]
    fn relative_output_joins_against_cwd_and_local_engine_nulls_revision() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let argv = strings(&["assemble", "--output", "build/inner", "-dBuildMode=debug"]);
        let got = capture_argv(&argv, tmp.path(), "enginerev0000000000000000");
        let joined = tmp
            .path()
            .join("build/inner")
            .to_string_lossy()
            .into_owned();
        assert_eq!(got.output_dir, joined);

        // A local-engine invocation hashes a null engine revision, so the
        // same defines give a different name than the stock revision.
        let local = strings(&[
            "--local-engine",
            "android_release",
            "--local-engine-src-path",
            "/engine/src",
            "assemble",
            "--output",
            "/out",
        ]);
        let got = capture_argv(&local, tmp.path(), "enginerev0000000000000000");
        assert_eq!(
            got.name,
            flutter_build_dir_name("", &got.defines, Path::new("/out"))
        );
        assert_ne!(
            got.name,
            flutter_build_dir_name("enginerev0000000000000000", &got.defines, Path::new("/out"))
        );
    }

    #[test]
    fn deferred_components_follows_targets_and_pubspec() {
        let tmp = tempfile::tempdir().expect("tempdir");
        fs::write(
            tmp.path().join("pubspec.yaml"),
            "name: app\nflutter:\n  deferred-components:\n    - name: big\n",
        )
        .expect("pubspec");
        let deferred = strings(&[
            "assemble",
            "-o",
            "/out",
            "android_aot_deferred_components_bundle_release_android-arm64",
        ]);
        let got = capture_argv(&deferred, tmp.path(), "e");
        assert_eq!(
            got.defines.get("DeferredComponents").map(String::as_str),
            Some("true")
        );

        // A debug target, or a pubspec without the section, keeps false.
        let debug = strings(&["assemble", "-o", "/out", "debug_android_application"]);
        assert_eq!(
            capture_argv(&debug, tmp.path(), "e")
                .defines
                .get("DeferredComponents")
                .map(String::as_str),
            Some("false")
        );
        fs::write(tmp.path().join("pubspec.yaml"), "name: app\n").expect("plain pubspec");
        assert_eq!(
            capture_argv(&deferred, tmp.path(), "e")
                .defines
                .get("DeferredComponents")
                .map(String::as_str),
            Some("false")
        );
    }

    /// The debug veto and the target check see only the COMMAND's
    /// positional targets — flutter iterates `argResults.rest` — so a
    /// stray token before `assemble` (here one containing `debug`, the
    /// shape the old whole-argv check misread into a veto) neither
    /// vetoes nor counts.
    #[test]
    fn deferred_components_checks_only_command_targets() {
        let tmp = tempfile::tempdir().expect("tempdir");
        fs::write(
            tmp.path().join("pubspec.yaml"),
            "name: app\nflutter:\n  deferred-components:\n    - name: big\n",
        )
        .expect("pubspec");
        let deferred = strings(&[
            "debug-tooling",
            "assemble",
            "-o",
            "/out",
            "android_aot_deferred_components_bundle_release_android-arm64",
        ]);
        assert_eq!(
            capture_argv(&deferred, tmp.path(), "e")
                .defines
                .get("DeferredComponents")
                .map(String::as_str),
            Some("true"),
            "a pre-command token containing `debug` is not a veto"
        );

        // A debug TARGET after the command still vetoes.
        let debug_target = strings(&[
            "assemble",
            "-o",
            "/out",
            "-dBuildMode=release",
            "debug_android_application",
            "android_aot_deferred_components_bundle_release_android-arm64",
        ]);
        assert_eq!(
            capture_argv(&debug_target, tmp.path(), "e")
                .defines
                .get("DeferredComponents")
                .map(String::as_str),
            Some("false")
        );
    }

    /// The pubspec declaration matches the manifest parser's key
    /// presence: `[]`, a trailing comment, and a bare colon all
    /// declare; a commented-out line and other sections do not.
    #[test]
    fn pubspec_deferred_components_accepts_every_declared_spelling() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let declared = [
            "name: app\nflutter:\n  deferred-components:\n    - name: big\n",
            "name: app\nflutter:\n  deferred-components: []\n",
            "name: app\nflutter:\n  deferred-components: # none yet\n",
        ];
        for pubspec in declared {
            fs::write(tmp.path().join("pubspec.yaml"), pubspec).expect("pubspec");
            assert!(
                pubspec_declares_deferred_components(tmp.path()),
                "declared: {pubspec:?}"
            );
        }
        let undeclared = [
            "name: app\nflutter:\n  # deferred-components:\n",
            "name: app\nflutter:\n  deferred-components-guide: x\n",
            "name: app\ntest:\n  deferred-components:\n",
            "name: app\nflutter:\n  uses-material-design: true\n",
            "name: app\n",
        ];
        for pubspec in undeclared {
            fs::write(tmp.path().join("pubspec.yaml"), pubspec).expect("pubspec");
            assert!(
                !pubspec_declares_deferred_components(tmp.path()),
                "undeclared: {pubspec:?}"
            );
        }
    }

    /// `--output=<value>` and `--output <value>` (and `-o <value>`)
    /// parse to the same capture — the split form is accepted wherever
    /// the joined form is (the audit's stale line pointed at a revision
    /// where it was not; this pins the equivalence).
    #[test]
    fn output_split_and_joined_forms_capture_identically() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let joined = strings(&[
            "assemble",
            "--output=/out/long",
            "-dBuildMode=release",
            "android_aot_bundle_release_android-arm64",
        ]);
        let split = strings(&[
            "assemble",
            "--output",
            "/out/long",
            "-dBuildMode=release",
            "android_aot_bundle_release_android-arm64",
        ]);
        assert_eq!(
            capture_templates(&joined, tmp.path(), "e", true).expect("joined"),
            capture_templates(&split, tmp.path(), "e", true).expect("split"),
            "both spellings yield the same captures"
        );
        assert_eq!(
            capture_argv(&split, tmp.path(), "e").output_dir,
            "/out/long"
        );

        // The short spelling in both forms agrees too.
        let short_split = strings(&["assemble", "-o", "/out/s", "-dA=b"]);
        let short_attached = strings(&["assemble", "-o/out/s", "-dA=b"]);
        assert_eq!(
            capture_argv(&short_split, tmp.path(), "e").output_dir,
            capture_argv(&short_attached, tmp.path(), "e").output_dir
        );
    }

    #[test]
    fn rejects_shapes_flutter_rejects() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let bad: [(&[&str], &str); 5] = [
            (&["assemble", "-dBuildMode=release"], "missing --output"),
            (
                &["assemble", "-o", "/out", "-dNOSEPARATOR"],
                "define without =",
            ),
            (&["assemble", "-o"], "option missing its value"),
            (
                &["assemble", "-o", "/out", "--DartDefines=%not%base64%"],
                "non-base64",
            ),
            (
                &["assemble", "-o", "/out", "--dart-define-from-file=env.json"],
                "dart-define-from-file",
            ),
        ];
        for (argv, why) in bad {
            let err = capture_templates(&strings(argv), tmp.path(), "e", true).expect_err(why);
            let kind = match err {
                ArgModelError::Unsupported(_) => "unsupported",
                ArgModelError::Invalid(_) => "invalid",
            };
            assert!(
                kind == "invalid" || why == "dart-define-from-file",
                "{why}: {err}"
            );
        }
    }
}
