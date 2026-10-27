//! The impact face's core: what a change to one file would
//! affect, over the analyzed workspace.
//!
//! The query assembles the impact substrate from the engine's public graph
//! faces and the dartq-index capture:
//!
//! * the **import closure** reads the engine's library import CSR (node =
//!   engine [`dartq_engine::LibraryId`] index);
//! * the **test graph** joins the CSR import edges with the
//!   defining-file basenames (dartq-index's derivation: imports primary,
//!   the `*_test.dart` convention fallback);
//! * the **file snapshot** folds the seed library's member files — blake3
//!   over the member hashes, the blake3-128 fold of the member API
//!   signatures (dartq-ai's [`dartq_ai::impact::fold_api_signatures`]), the
//!   library-face declaration names with their member-level signature
//!   digests (#1561: the canonical per-name slices, derived on demand from
//!   the resident element model — never persisted). Per member the
//!   persisted-facts hint decides the read face: a snapshot hit is
//!   `materialize`d through the #341 load faces; a genuine miss reads the
//!   engine's resident facts through the #341 accessor face
//!   ([`dartq_engine::Engine::file_facts`]) — the analysis prelude has
//!   already computed them, so no member is ever re-scanned (the retired
//!   scan+capture seed also carried a language-version drift: it re-fixed
//!   the default version where the engine settles each file's own).
//!
//! One-shot posture: a CLI run holds no baseline, so the seed enters the
//! query as a **hypothetical** API-level change ([`LibraryChange::hypothetical`])
//! — the report answers "what depends on this file's API surface". The
//! daemon passes real before/after diffs through the same query.
//!
//! The **environment matrix** (#1554) rides the compat face's context
//! substrate: when the workspace's `dartq.yaml` declares SDK contexts, the
//! one-shot run answers one ✓/✗ row per context — the affected set
//! re-analyzed under each context's SDK, one axis engine alive at a time
//! (the bounded-memory doctrine), compatible when zero ERROR-severity
//! diagnostics land on the affected files (the compat verdict rule). The
//! resolution is the same [`crate::config`] context model the compat face
//! uses, so both faces answer contexts from one fact source. A contextless
//! workspace answers one honest `default` row — the same verdict rule
//! evaluated over the main pass's diagnostics, so a target (or its
//! affected set) carrying an ERROR never ships an unevaluated
//! affirmative.
//!
//! [`LibraryChange::hypothetical`]: dartq_ai::impact::LibraryChange::hypothetical

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use dartq_ai::impact::{self as query, FileSnapshot, LibraryChange, content_hash};
use dartq_diagnostics::{CodeRegistry, Diagnostic, Severity};
use dartq_engine::state::persist::PersistedFacts;
use dartq_engine::{CsrGraph, Engine, EngineConfig, LibraryId, memory_store, semdiag};
use dartq_index::{DeclRow, TestGraph};
use dartq_package::PackageTable;
use dartq_pub::{PackageConfig, ResolvedPackageGraph};
use dartq_sdk::SdkSnapshot;
use dartq_source::{FileId, FileTable, StringId, StringTable};
use rustc_hash::{FxHashMap, FxHashSet};
use thiserror::Error;

use crate::analyze::AnalyzeError;
use crate::workspace::{embedder_lib_dir, package_assembly_strict};

/// Why an impact run aborts: target problems are the CLI's usage tier,
/// prelude failures the internal tier, and the environments axis carries
/// the same split as the compat face — a malformed config, an unresolvable
/// context locator, or an unusable axis workspace refuses as usage; a
/// failed axis engine is internal.
#[derive(Debug, Error)]
pub enum ImpactError {
    /// The target path does not exist (the message names the path as given).
    #[error("Directory or file doesn't exist: {0}")]
    Missing(PathBuf),
    /// The canonical target is not an analyzed `.dart` file of the
    /// workspace.
    #[error("not an analyzed .dart file: {0}")]
    NotAnalyzed(PathBuf),
    /// The discover+analyze prelude failed.
    #[error(transparent)]
    Step(#[from] AnalyzeError),
    /// The environments axis hit an unusable input — a malformed
    /// `dartq.yaml`, an unresolvable context locator, or an unusable axis
    /// workspace/engine. The message body is the usage-tier text (the
    /// compat face's phrasing).
    #[error("{0}")]
    Usage(String),
    /// An environment axis's discovery or analysis failed.
    #[error("{0}")]
    Internal(String),
}

/// The JSON face: one object — the report's serialized shape (the report
/// carries its own `schema_version`). The payload is routed through
/// `dartq_schema::validate(.., Domain::Impact)` right here — the
/// producer-side gate the index face holds, shared verbatim by the CLI and
/// the daemon (both call this shaper).
///
/// Tier-invariant (the canonical verbosity tables): every field is a
/// locator or a verdict — `changed[].signature_changed` and
/// `affected[].impact` are verdicts the published v2 schema requires at
/// every tier — so the CLI's `--verbosity` parses for grammar parity and
/// shapes nothing here (the index run summary's accept-and-document
/// posture).
///
/// # Panics
///
/// Panics when the born payload violates the published schema — a
/// producer bug, surfaced never clamped (the CLI's crash tier turns it
/// into exit 4 with stdout kept empty; the daemon's read task contains
/// it at its panic boundary).
#[must_use]
pub fn to_json(report: &query::ImpactReport) -> serde_json::Value {
    let value = serde_json::to_value(report).expect("ImpactReport serializes");
    let text = value.to_string();
    dartq_schema::validate(&text, dartq_schema::Domain::Impact)
        .expect("the impact report is schema-valid at birth");
    value
}

/// Runs the impact face over an assembled engine: the discover+analyze
/// prelude (the pass's diagnostics are the environment verdicts' input),
/// the report for `target`'s owning library as a hypothetical API-level
/// change, then the environment section — the `dartq.yaml` contexts' ✓/✗
/// rows over the affected set, or the one honest `default` row when the
/// workspace declares none ([`environments_section`]).
///
/// # Errors
///
/// [`ImpactError`] on a missing or unanalyzed target, discovery failure,
/// cancellation, or an unusable context configuration.
pub fn impact(
    engine: &mut Engine,
    target: &Path,
    symbols: &[String],
) -> Result<query::ImpactReport, ImpactError> {
    // The run's diagnostics move out here: the borrowed `AnalyzeRun` drops
    // with the statement, so the engine re-borrows for the report.
    let diagnostics = crate::analyze::run_overlaid(engine, &[], &[])
        .map_err(ImpactError::from)?
        .diagnostics;
    let mut report = impact_after_analysis_with_symbols(engine, target, symbols)?;
    report.affected_environments =
        environments_after_analysis(engine, target, &report, &diagnostics)?;
    Ok(report)
}

/// The environment section over an already-analyzed engine — the one
/// implementation both faces answer through (#2657): the CLI's
/// [`impact`] computes it over its own pass's rows, the daemon's
/// `dartq/impact` over the session's analyzed-view rows. One row per
/// declared `dartq.yaml` context, each judged by the compat verdict rule
/// over the affected set; a workspace with no `dartq.yaml` — or one
/// without contexts — answers the single `default` row, derived from
/// `diagnostics`, never an unevaluated affirmative.
///
/// # Errors
///
/// [`ImpactError::Usage`] on a malformed config, an unresolvable locator,
/// or an unusable axis workspace/engine; [`ImpactError::Internal`] on a
/// failed axis discovery or analysis.
pub fn environments_after_analysis(
    engine: &Engine,
    target: &Path,
    report: &query::ImpactReport,
    diagnostics: &[Diagnostic],
) -> Result<Vec<query::EnvironmentImpact>, ImpactError> {
    environments_section(engine, target, report, diagnostics)
}

/// The environment section: one honest row per declared `dartq.yaml`
/// context, each judged by the compat face's verdict rule over the
/// affected set ([`compatible_over_affected`]); a workspace with no
/// `dartq.yaml` — or one without contexts — answers the single `default`
/// row, the same rule evaluated over `diagnostics` (the main analysis
/// pass's rows), so the contextless answer is derived, never an
/// unevaluated affirmative.
///
/// The context resolution rides the same substrate the compat face uses
/// ([`crate::config`]'s context model and locator resolution, the strict
/// package assembly, the per-axis engine, one semantic graph alive at a
/// time), so both faces answer contexts from one fact source; an SDK root
/// that fails to load isolates as an incompatible row (the compat
/// doctrine), never fatal.
///
/// The v1 row shape is the ✓/✗ pair alone: the failure detail a row
/// cannot carry is one `dartq compat` run away.
///
/// # Errors
///
/// [`ImpactError::Usage`] on a malformed config, an unresolvable locator,
/// or an unusable axis workspace/engine; [`ImpactError::Internal`] on a
/// failed axis discovery or analysis.
fn environments_section(
    engine: &Engine,
    target: &Path,
    report: &query::ImpactReport,
    diagnostics: &[Diagnostic],
) -> Result<Vec<query::EnvironmentImpact>, ImpactError> {
    let canonical = target
        .canonicalize()
        .map_err(|_| ImpactError::Missing(target.to_path_buf()))?;
    let Some(seed_file) = engine.file_table().try_file_id(&canonical) else {
        return Err(ImpactError::NotAnalyzed(canonical));
    };
    let Some(seed) = engine.library_of(seed_file) else {
        return Err(ImpactError::NotAnalyzed(canonical));
    };
    let affected = affected_label_set(engine, seed.index(), report);

    // The declared contexts: the nearest `dartq.yaml` up the target's root
    // chain (the workspace root's file, the config face's contract). No
    // file, no axes — the zero-config fast path answers the default row
    // from the already-run analysis.
    let Some(config_root) = canonical
        .parent()
        .and_then(|dir| dir.ancestors().find(|dir| dir.join("dartq.yaml").is_file()))
    else {
        return Ok(vec![default_environment_row(
            engine,
            &affected,
            diagnostics,
        )]);
    };
    let contexts = crate::config::load(config_root)
        .map_err(|error| ImpactError::Usage(error.to_string()))?
        .map(|config| config.contexts)
        .unwrap_or_default();
    if contexts.is_empty() {
        return Ok(vec![default_environment_row(
            engine,
            &affected,
            diagnostics,
        )]);
    }

    // The pub faces are axis-independent: built once, every axis's engine
    // reassembles from them. The assembly's one stale-config advisory has
    // no channel on this machine face; it never gates the matrix.
    let (config, graph, config_dir, _) = package_assembly_strict(config_root)
        .map_err(|error| ImpactError::Usage(error.to_string()))?;
    let mut rows = Vec::with_capacity(contexts.len());
    for context in &contexts {
        let root = crate::config::resolve_locator(&context.label, &context.sdk, config_root)
            .map_err(|error| ImpactError::Usage(error.to_string()))?;
        let compatible = analyze_environment(&root, &config, &graph, &config_dir, &affected)?;
        rows.push(query::EnvironmentImpact {
            label: context.label.clone(),
            compatible,
        });
    }
    Ok(rows)
}

/// The contextless answer: the one `default` row, its verdict the compat
/// rule over `diagnostics`.
fn default_environment_row(
    engine: &Engine,
    affected: &FxHashSet<String>,
    diagnostics: &[Diagnostic],
) -> query::EnvironmentImpact {
    query::EnvironmentImpact {
        label: "default".to_owned(),
        compatible: compatible_over_affected(engine, affected, diagnostics),
    }
}

/// The affected set as stable labels: the seed library's member files plus
/// every affected library's member files — the same located spellings the
/// report's Affected section already names (the one [`crate::pkg_label::locate`]
/// projection on both sides, so the string set matches whatever the report
/// spells). Label strings, not file ids: the environment axes run their own
/// engines, whose id spaces are their own.
fn affected_label_set(
    engine: &Engine,
    seed: u32,
    report: &query::ImpactReport,
) -> FxHashSet<String> {
    let affected_labels: FxHashSet<&str> = report
        .affected
        .iter()
        .map(|row| row.path.as_str())
        .collect();
    let mut affected_files: FxHashSet<String> = FxHashSet::default();
    let root = engine.packages().workspace_root();
    let label = |path| crate::pkg_label::locate(root, path);
    for (library, files) in engine.libraries() {
        let affected_library = library.index() == seed
            || files
                .first()
                .and_then(|&file| engine.file_table().path(file))
                .is_some_and(|path| affected_labels.contains(label(path).as_str()));
        if !affected_library {
            continue;
        }
        affected_files.extend(
            files
                .iter()
                .filter_map(|&file| engine.file_table().path(file))
                .map(label),
        );
    }
    affected_files
}

/// The compat verdict rule, stated once for both impact faces' rows (the
/// contextless default and the per-context axes): the judged set is
/// compatible when zero ERROR-severity diagnostics land on it — warnings
/// and lints never flip, and diagnostics outside the set are nobody
/// else's verdict (the whole-workspace matrix is the compat face's job).
/// `diagnostics` are one engine's merged analysis rows.
fn compatible_over_affected(
    engine: &Engine,
    affected: &FxHashSet<String>,
    diagnostics: &[Diagnostic],
) -> bool {
    let root = engine.packages().workspace_root();
    diagnostics.iter().all(|diag| {
        diag.severity != Severity::Error
            || !engine.file_table().path(diag.file).is_some_and(|path| {
                // The engine's table builds from the same resolution as
                // the label set's own projection, so the spellings match.
                affected.contains(&crate::pkg_label::locate(root, path))
            })
    })
}

/// One environment axis, the compat face's recipe: assemble the engine
/// under `root`'s snapshot over the shared package resolution, run the
/// discover+analyze passes, and answer whether the affected set produced
/// zero ERROR-severity diagnostics (the compat verdict rule — warnings and
/// lints never flip an environment). An SDK root that fails to load
/// isolates as `false` (the compat doctrine; the v1 row has no detail
/// column — `dartq compat` names the cause). The engine drops at return —
/// one semantic graph alive at a time.
fn analyze_environment(
    root: &Path,
    config: &Arc<PackageConfig>,
    graph: &ResolvedPackageGraph,
    config_dir: &Path,
    affected: &FxHashSet<String>,
) -> Result<bool, ImpactError> {
    // The environment's snapshot rides the resolution's embedder table
    // when the shared config carries a `sky_engine` package (the default
    // context's posture, the compat face's rule); an unusable embedder
    // yaml degrades to the plain dart-sdk table.
    let loaded = match embedder_lib_dir(config, config_dir) {
        Some(lib_dir) => {
            SdkSnapshot::load(root, Some(&lib_dir)).or_else(|_| SdkSnapshot::load(root, None))
        }
        None => SdkSnapshot::load(root, None),
    };
    let Ok(snapshot) = loaded else {
        return Ok(false);
    };
    let packages = PackageTable::build(config, graph, config_dir)
        .map_err(|error| ImpactError::Usage(error.to_string()))?;
    let mut engine = Engine::new(EngineConfig {
        store: memory_store(),
        sdk: snapshot,
        packages,
        pub_config: Arc::clone(config),
        config_dir: config_dir.to_path_buf(),
        // The semantic families are part of the verdict (the compat face's
        // rationale): the SDK-difference signal rides the default diagnoser.
        semdiag: Some(semdiag::diagnoser()),
        target: dartq_engine::DEFAULT_TARGET.to_owned(),
        declared_variables: rustc_hash::FxHashMap::default(),
        experiments: Vec::new(),
    })
    .map_err(|error| ImpactError::Usage(format!("engine context unusable: {error}")))?;
    engine
        .discover_workspace()
        .map_err(|error| ImpactError::Internal(format!("discovery failed: {error}")))?;
    let diags = engine
        .analyze(
            &[],
            &mut |_: dartq_engine::EngineEvent| {},
            &dartq_engine::CancellationToken::default(),
        )
        .map_err(|_: dartq_engine::Cancelled| ImpactError::Internal("analysis cancelled".into()))?;
    Ok(compatible_over_affected(&engine, affected, &diags))
}

/// The report over an already-analyzed engine — the daemon path, whose read
/// task has just run the analysis pass. Hint-less: every member seeds. A
/// caller holding a snapshot generation threads it through
/// [`impact_after_analysis_with_persisted_facts`].
///
/// # Errors
///
/// [`ImpactError::Missing`] / [`ImpactError::NotAnalyzed`] on a bad target.
pub fn impact_after_analysis(
    engine: &Engine,
    target: &Path,
) -> Result<query::ImpactReport, ImpactError> {
    impact_after_analysis_with_persisted_facts(engine, target, &unpersisted, &[])
}

/// [`impact_after_analysis`] with the `--symbol` narrowing: the CLI's
/// report path. `symbols` is the raw flag list — empty is the file-level
/// posture (backward compatible).
///
/// # Errors
///
/// [`ImpactError::Missing`] / [`ImpactError::NotAnalyzed`] on a bad target;
/// [`ImpactError::Usage`] when a supplied name resolves to no face of the
/// seed library.
pub fn impact_after_analysis_with_symbols(
    engine: &Engine,
    target: &Path,
    symbols: &[String],
) -> Result<query::ImpactReport, ImpactError> {
    impact_after_analysis_with_persisted_facts(engine, target, &unpersisted, symbols)
}

/// The `dartq test --impacted` selection — the one implementation the CLI's
/// test faces and the daemon's `dartq/test/run` share (the daemon spawns a
/// `dartq test` child, whose selection this is): the changed `_test.dart`
/// files plus every suite the semantic impact graph reports for the changed
/// libraries, as root-relative `/`-joined spellings.
///
/// `Err(reason)` is the conservative fallback — run the full selection —
/// taken when the change cannot be bounded: no repository (or `base` does
/// not resolve), a change whose blast radius no selection can approximate
/// (a pubspec, an options file, generated code), or a workspace whose
/// semantic graph cannot assemble. The reason is the diagnostic the caller
/// prints (the CLI's `dartq test: {reason} — running everything` line);
/// there is no narrower heuristic to fall back to — the retired
/// same-package-tests guess is gone, and the fallback is the full run,
/// never a guess.
///
/// `sdk` is the SDK root override ([`open_engine`]'s `None` = discovery);
/// production callers pass `None` — the parameter exists so a pinned
/// assembly is possible (and testable).
///
/// [`open_engine`]: crate::workspace::open_engine
pub fn impacted_tests(root: &Path, sdk: Option<&Path>, base: &str) -> Result<Vec<String>, String> {
    let changed = changed_since(root, base)
        .ok_or_else(|| format!("cannot diff against {base} (no git repository?)"))?;
    let mut selected: BTreeSet<PathBuf> = BTreeSet::new();
    let mut sources = Vec::new();
    for relative in &changed {
        let path = if relative.is_absolute() {
            relative.clone()
        } else {
            root.join(relative)
        };
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        if matches!(
            name,
            "pubspec.yaml"
                | "pubspec.lock"
                | "analysis_options.yaml"
                | "build.yaml"
                | "dartq.yaml"
                | "dart_test.yaml"
        ) || name.ends_with(".g.dart")
            || name.ends_with(".freezed.dart")
            || name.ends_with(".mocks.dart")
        {
            return Err(format!("{} changed", path.display()));
        }
        if name.ends_with(".dart") {
            if name.ends_with("_test.dart") && path.is_file() {
                selected.insert(path.clone());
            }
            sources.push(path);
        }
    }
    if !sources.is_empty() {
        let tests = (|| -> Result<Vec<PathBuf>, String> {
            let (mut engine, _dir) = crate::workspace::open_engine(root, sdk)
                .map_err(|error| format!("cannot assemble the workspace: {error}"))?;
            crate::analyze::run_overlaid(&mut engine, &[], &[])
                .map_err(|error| format!("the analysis pass failed: {error}"))?;
            // The report's test paths spell through the file locator
            // (root-relative inside the analysis root, absolute outside),
            // so a relative tail resolves back against the root that
            // stripped it — the engine's own workspace root, the one base
            // every row's spelling was derived from.
            let analysis_root = engine.packages().workspace_root().to_path_buf();
            let mut tests = Vec::new();
            for seed in &sources {
                let report = impact_after_analysis(&engine, seed).map_err(|error| {
                    format!(
                        "{} is outside the analyzed workspace: {error}",
                        seed.display()
                    )
                })?;
                tests.extend(report.affected_tests.into_iter().map(|test| {
                    let path = PathBuf::from(test.path);
                    if path.is_absolute() {
                        path
                    } else {
                        analysis_root.join(path)
                    }
                }));
            }
            Ok(tests)
        })();
        match tests {
            Ok(tests) => selected.extend(tests),
            Err(reason) => return Err(reason),
        }
    }
    Ok(selected
        .into_iter()
        .filter_map(|path| {
            path.strip_prefix(root)
                .ok()
                .map(|rel| rel.to_string_lossy().replace('\\', "/"))
        })
        .filter(|rel| rel.ends_with("_test.dart"))
        .collect())
}

/// Files that differ from `base` in the working tree — committed since,
/// staged, unstaged, and untracked (not ignored) — relative to `root`.
/// `None` when there is no repository or `base` does not resolve.
fn changed_since(root: &Path, base: &str) -> Option<Vec<PathBuf>> {
    let git = |args: &[&str]| -> Option<Vec<PathBuf>> {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()?;
        out.status.success().then(|| {
            out.stdout
                .split(|&b| b == 0)
                .filter(|path| !path.is_empty())
                .map(|path| PathBuf::from(String::from_utf8_lossy(path).into_owned()))
                .collect()
        })
    };
    let mut changed = git(&["diff", "--name-only", "-z", "--relative", base, "--"])?;
    changed.extend(git(&["ls-files", "-z", "--others", "--exclude-standard"])?);
    Some(changed)
}

/// The persisted-facts hint the query consults before seeding (the #341
/// load faces): called once per member file with the file's resident
/// source, it answers `Some` when the snapshot generation carries the
/// file's facts under its current content key and `None` on a genuine miss
/// — first run, generation mismatch, tables-stamp mismatch, eviction. The
/// accelerator invariant: a miss is a recompute (the seed arm), never an
/// error.
pub type PersistedFactsHint<'a> = &'a dyn Fn(&str) -> Option<PersistedFacts>;

/// The hint-less posture.
fn unpersisted(_: &str) -> Option<PersistedFacts> {
    None
}

/// [`impact_after_analysis`] over a persisted-facts hint: members the
/// snapshot carries surface through [`PersistedFacts::materialize`], the
/// rest seed exactly as before — the outputs are byte-identical either way
/// for the same content (the impact equivalence test). The environment
/// section answers the single `default` row, its verdict the compat rule
/// over the diagnostics this entry carries — none (the caller's analysis
/// pass kept them), so the rule evaluates over an empty set; the CLI path
/// re-answers the section with the pass's rows.
///
/// `symbols` is the CLI's `--symbol` list: when non-empty, the seed
/// narrows from the whole face set to the resolved hits. Each spelling
/// resolves against the seed library's own declarations (#2621):
///
/// * `name` — the union of every same-named declaration (the library
///   face `name` itself, plus every container declaring a member or
///   constructor named `name`);
/// * `Container.name` — that container's member (a constructor selector
///   included), narrowing to the container's face (the signature fold is
///   per-face, never per-member, so `member` narrows nothing past its
///   container);
/// * `path/to/file.dart:line` — the declaration whose name token starts
///   on that line of a seed-library member file.
///
/// Every supplied name must hit: a miss is the usage tier, not a silent
/// file-level fallback. The propagation set the affected rows'
/// `reference_lines` collect against is the narrowed (or full, under an
/// empty `symbols`) seed-name set — a `(target_file, target_name)` pair
/// whose file is a member of the seed library.
///
/// # Errors
///
/// [`ImpactError::Missing`] / [`ImpactError::NotAnalyzed`] on a bad target.
/// [`ImpactError::Usage`] when a `--symbol` resolves to no face name of
/// the seed library.
///
/// # Panics
///
/// Panics when the seed library's member list is missing from the live
/// library set — an internal invariant breach (the seed resolves through
/// the same set).
pub fn impact_after_analysis_with_persisted_facts(
    engine: &Engine,
    target: &Path,
    persisted: PersistedFactsHint<'_>,
    symbols: &[String],
) -> Result<query::ImpactReport, ImpactError> {
    // The seed: the target file's owning library.
    let canonical = target
        .canonicalize()
        .map_err(|_| ImpactError::Missing(target.to_path_buf()))?;
    let Some(seed_file) = engine.file_table().try_file_id(&canonical) else {
        return Err(ImpactError::NotAnalyzed(canonical));
    };
    let Some(seed_library) = engine.library_of(seed_file) else {
        return Err(ImpactError::NotAnalyzed(canonical));
    };

    // Library identities and the seed's member list: one `libraries` fetch
    // feeds both the member lookup and the substrate's labels. Borrowed,
    // not cloned: the list only feeds the immutable surface fold below,
    // and `libraries` outlives every use in this function.
    let libraries = engine.libraries();
    let seed_library_index = seed_library.index() as usize;
    let members = libraries
        .iter()
        .find(|(library, _)| library.index() as usize == seed_library_index)
        .map(|(_, files)| files.as_slice())
        .expect("live seed library");

    // One intern table for the whole answer: the engine's own — resident
    // facts' `StringId`s are native to it, and materializing a payload
    // re-interns into it idempotently (Mutex-guarded `intern`, the
    // documented query posture: the only side effect is interning).
    let strings = engine.strings();
    let mut restored = FileTable::new();
    let mut current = library_surface(engine, members, strings, persisted, &mut restored);

    // The propagation set the affected rows' `reference_lines` collect
    // against — the `(member file, face name)` pairs the seed's surfaces
    // declare. Under `--symbol` the set narrows to the resolved hits; the
    // three spellings (#2621) — bare `name` (the union of every
    // same-named declaration), `Container.name` (that container's
    // member), and `path/to/file.dart:line` (the declaration at that
    // line) — resolve against the seed library's captures below.
    let seed_names: FxHashSet<StringId> = if symbols.is_empty() {
        current.member_names.iter().copied().collect()
    } else {
        let mut resolved = FxHashSet::default();
        for symbol in symbols {
            resolved.extend(resolve_symbol_spelling(
                engine,
                members,
                strings,
                &current.member_names,
                symbol,
                &canonical,
            )?);
        }
        resolved
    };
    // Narrow the seed's member face to the resolved hits: the
    // parallel (names, digests) columns stay ascending — the filter keeps
    // the sort order and the pairwise alignment intact. File-level posture
    // skips the step entirely.
    if !symbols.is_empty() {
        let keep: Vec<(StringId, [u8; 16])> = current
            .member_names
            .iter()
            .copied()
            .zip(current.member_digests.iter().copied())
            .filter(|(name, _)| seed_names.contains(name))
            .collect();
        current.member_names = keep.iter().map(|(name, _)| *name).collect();
        current.member_digests = keep.into_iter().map(|(_, digest)| digest).collect();
    }

    // The seed-name→file map: each `(file, name)` pair is a declaring
    // member's library-face row — the RefRow's `(target_file, target_name)`
    // key shape.
    let mut propagation: FxHashSet<(FileId, StringId)> = FxHashSet::default();
    for &file in members {
        let capture = engine.file_capture(file).expect("resident member");
        for row in capture.index.decls() {
            if row.is_library_face() && seed_names.contains(&row.name) {
                propagation.insert((file, row.name));
            }
        }
    }

    // The substrate over the engine's faces, then the query.
    let substrate = impact_substrate(engine, &libraries);
    let surfaces: FxHashMap<u32, &dartq_index::LibraryIndex> = engine
        .library_surfaces()
        .map(|(library, surface)| (library.index(), surface))
        .collect();
    let defining_of: FxHashMap<u32, FileId> = libraries
        .iter()
        .filter_map(|(library, files)| files.first().map(|&f| (library.index(), f)))
        .collect();
    let reference_lines_of = |library: u32| -> Vec<u32> {
        let (Some(surface), Some(&defining)) = (surfaces.get(&library), defining_of.get(&library))
        else {
            return Vec::new();
        };
        let mut lines = Vec::new();
        for row in surface.refs() {
            if row.file != defining || !propagation.contains(&(row.target_file, row.target_name)) {
                continue;
            }
            let (Some(index), Some(content)) =
                (engine.line_index(row.file), engine.content(row.file))
            else {
                continue;
            };
            let Ok(text) = std::str::from_utf8(content) else {
                continue;
            };
            lines.push(index.line_col_utf16(text, row.range.offset).0);
        }
        lines.sort_unstable();
        lines.dedup();
        lines
    };
    let change = LibraryChange::hypothetical(seed_library.index(), current);
    let mut report = query::impact(
        substrate.node_count(),
        |library| substrate.neighbors(library),
        |library| substrate.label(library),
        &substrate.tests,
        strings,
        &[change],
        reference_lines_of,
    );
    // The locator envelope: the root rides exactly when a file-bearing
    // section carries a row (a faceless seed library answers an all-empty
    // report — no file field, no root to disambiguate).
    if !(report.changed.is_empty()
        && report.affected.is_empty()
        && report.affected_tests.is_empty())
    {
        report.root = Some(engine.packages().workspace_root().display().to_string());
    }
    let affected = affected_label_set(engine, seed_library.index(), &report);
    report.affected_environments = vec![default_environment_row(engine, &affected, &[])];
    Ok(report)
}

/// One `--symbol` spelling resolved to the seed library's face names
/// (#2621). The signature fold is per face, so every spelling lands on
/// whole face names — a bare `name` (the union of every same-named
/// declaration), a `Container.name` (that container's member), or a
/// `path/to/file.dart:line` (the declaration at that line). A spelling
/// that resolves to nothing is [`ImpactError::Usage`] — never a silent
/// file-level fallback.
fn resolve_symbol_spelling(
    engine: &Engine,
    members: &[FileId],
    strings: &StringTable,
    faces: &[StringId],
    symbol: &str,
    seed: &Path,
) -> Result<Vec<StringId>, ImpactError> {
    let usage = || ImpactError::Usage(format!("no such symbol in {}: {symbol}", seed.display()));
    let candidates = if let Some((path, line)) = file_line_of(symbol) {
        declaration_at(engine, members, path, line, seed)
    } else if let Some((container, member)) = symbol.split_once('.') {
        member_of(engine, members, strings, faces, container, member)
    } else {
        bare_name(engine, members, strings, faces, symbol)
    };
    let mut resolved = candidates.ok_or_else(usage)?;
    // The face-membership invariant: every resolved name must be one of
    // the seed's faces — a file:line walk that lands on a faceless row
    // (an unnamed extension) resolves to nothing.
    resolved.retain(|name| faces.binary_search(name).is_ok());
    resolved.sort_unstable();
    resolved.dedup();
    if resolved.is_empty() {
        return Err(usage());
    }
    Ok(resolved)
}

/// The `path/to/file.dart:line` spelling test: the last colon separates a
/// 1-based line from the path. `None` for anything else — bare names and
/// `Container.name` spellings never carry a numeric tail.
fn file_line_of(symbol: &str) -> Option<(&str, u32)> {
    let (path, line) = symbol.rsplit_once(':')?;
    let line = line.parse::<u32>().ok()?;
    (line >= 1).then_some((path, line))
}

/// The bare `name` spelling: the union of every same-named declaration —
/// the library-scope face `name` itself, plus every container declaring a
/// member (any row kind, type parameters included) or a constructor
/// selector named `name`. Constructors mint no capture rows, so the
/// selector scan reads the element face.
fn bare_name(
    engine: &Engine,
    members: &[FileId],
    strings: &StringTable,
    faces: &[StringId],
    name: &str,
) -> Option<Vec<StringId>> {
    let name_id = strings.intern(name);
    let mut resolved = Vec::new();
    if faces.binary_search(&name_id).is_ok() {
        resolved.push(name_id);
    }
    for &file in members {
        let capture = engine.file_capture(file).expect("resident member");
        let surface = capture.index.decls();
        if let Some(rows) = capture.index.by_name(name_id) {
            resolved.extend(
                rows.iter()
                    .filter(|row| row.container != DeclRow::NONE)
                    .map(|row| surface[row.container as usize].name),
            );
        }
        let facts = engine.file_facts(file).expect("resident member facts");
        for &container in facts.elements.container_order() {
            if facts
                .elements
                .container(container)
                .is_some_and(|element| element.constructors.contains(&name_id))
            {
                resolved.push(container);
            }
        }
    }
    (!resolved.is_empty()).then_some(resolved)
}

/// The `Container.name` spelling: the container must be a face of the
/// seed library and the member — a captured member row (any kind) or a
/// constructor selector, the unnamed face included — must declare inside
/// it. The fold is per face, so the resolution is the container's face
/// name.
fn member_of(
    engine: &Engine,
    members: &[FileId],
    strings: &StringTable,
    faces: &[StringId],
    container: &str,
    member: &str,
) -> Option<Vec<StringId>> {
    let container_id = strings.intern(container);
    if faces.binary_search(&container_id).is_err() {
        return None;
    }
    let member_id = strings.intern(member);
    for &file in members {
        let capture = engine.file_capture(file).expect("resident member");
        let surface = capture.index.decls();
        let member_row = capture.index.by_name(member_id).is_some_and(|rows| {
            rows.iter().any(|row| {
                row.container != DeclRow::NONE
                    && surface[row.container as usize].name == container_id
            })
        });
        if member_row {
            return Some(vec![container_id]);
        }
        let facts = engine.file_facts(file).expect("resident member facts");
        if facts
            .elements
            .container(container_id)
            .is_some_and(|element| {
                element.constructors.contains(&member_id)
                    || (member_id == container_id && element.has_unnamed_constructor)
            })
        {
            return Some(vec![container_id]);
        }
    }
    None
}

/// The `path/to/file.dart:line` spelling: the declarations whose name
/// tokens start on `line` of the member file at `path`, resolved to their
/// face names (the enclosing container's for a member). A relative `path`
/// resolves as given (cwd, like any path argument), then against the
/// assembly's workspace root (the base the target itself resolves
/// against), then against the target file's directory; the file must be a
/// member of the seed library. Constructor names mint no capture rows and
/// carry no spans in the element model, so a line cannot select one —
/// the `Container.name` spelling names constructors.
fn declaration_at(
    engine: &Engine,
    members: &[FileId],
    path: &str,
    line: u32,
    seed: &Path,
) -> Option<Vec<StringId>> {
    let resolved = |joined: &Path| -> Option<FileId> {
        engine
            .file_table()
            .try_file_id(&joined.canonicalize().ok()?)
    };
    let file = resolved(Path::new(path))
        .or_else(|| resolved(&engine.packages().workspace_root().join(path)))
        .or_else(|| seed.parent().and_then(|dir| resolved(&dir.join(path))))?;
    if !members.contains(&file) {
        return None;
    }
    let capture = engine.file_capture(file).expect("resident member");
    let surface = capture.index.decls();
    let index = engine.line_index(file).expect("resident member line index");
    let content = engine.content(file).expect("resident member content");
    let text = std::str::from_utf8(content).expect("engine content is UTF-8");
    let mut faces_at = Vec::new();
    for row in surface {
        if index.line_col_utf16(text, row.range.offset).0 != line {
            continue;
        }
        // Walk the container anchors up to the declaring face (top-level
        // rows carry no anchor; the walk is bounded by nesting depth).
        let mut face = row;
        while face.container != DeclRow::NONE {
            face = &surface[face.container as usize];
        }
        faces_at.push(face.name);
    }
    (!faces_at.is_empty()).then_some(faces_at)
}

/// The graph substrate both graph faces read, built once per call (#558):
/// the engine's import CSR projection, the arena-spaced located-path
/// labels, and the test graph joined over them.
pub(crate) struct Substrate {
    csr: CsrGraph,
    /// The located spelling of every arena slot's defining file — projected
    /// once per build through [`crate::pkg_label::locate`] against the
    /// engine's workspace root — `None` on a dead slot. Sized by
    /// [`CsrGraph::nodes`] (the arena's slot count), never by the
    /// live-library count: the consumers iterate the CSR's node space,
    /// which includes retired slots, so a live-count-sized table indexes
    /// out of bounds on the first retirement (#558 P1).
    idents: Vec<Option<String>>,
    tests: TestGraph,
}

/// Assembles the substrate over `engine`'s faces and the already-fetched
/// `libraries` (the faces pass the same fetch they need for their own
/// member lookups — no second `Engine::libraries` build).
///
/// The labels are the report assembly boundary's located spellings (the
/// lon/text file locator: root-relative inside the analysis root, absolute
/// outside), constructed once per live library here; a path that is not
/// UTF-8 labels lossily — it cannot ride the JSON faces' string columns
/// byte-exact anyway. The test graph's convention fallback reads the
/// label's basename and directory, which the locator preserves (only the
/// prefix spelling changes, never the tail's shape).
pub(crate) fn impact_substrate(
    engine: &Engine,
    libraries: &[(LibraryId, Vec<FileId>)],
) -> Substrate {
    let csr = engine.library_import_graph();
    let count = csr.nodes();
    let root = engine.packages().workspace_root();
    let mut idents: Vec<Option<String>> = vec![None; count];
    for (library, files) in libraries {
        if let Some(label) = files
            .first()
            .and_then(|&file| engine.file_table().path(file))
            .map(|path| crate::pkg_label::locate(root, path))
        {
            idents[library.index() as usize] = Some(label);
        }
    }
    let neighbors = |library: u32| -> &[u32] {
        let start = csr.offsets()[library as usize] as usize;
        let end = csr.offsets()[library as usize + 1] as usize;
        &csr.edges()[start..end]
    };
    // Dead slots carry no identity and gain no test edges — the convention
    // fallback reads the identity path, and `None` skips the slot.
    let tests = TestGraph::build(count, neighbors, |library| {
        idents[library as usize].as_deref()
    });
    Substrate { csr, idents, tests }
}

impl Substrate {
    /// The node count: the engine library arena's slot count, dead slots
    /// included (the CSR's node space).
    #[must_use]
    pub(crate) fn node_count(&self) -> usize {
        self.csr.nodes()
    }

    /// The resolved import targets of `library` (library ids, not slots).
    #[must_use]
    pub(crate) fn neighbors(&self, library: u32) -> &[u32] {
        let start = self.csr.offsets()[library as usize] as usize;
        let end = self.csr.offsets()[library as usize + 1] as usize;
        &self.csr.edges()[start..end]
    }

    /// The defining file's located spelling of `library` (root-relative
    /// inside the analysis root, absolute outside — see [`impact_substrate`]).
    ///
    /// Dead slots never reach here — the label consumers are flood-joined:
    /// a retired slot projects no edges, so it appears in no importer row
    /// (`RevImportGraph` inverts `neighbors`, and nothing lists a slot with
    /// an empty row as an importer), builds no test edge (`TestGraph::build`
    /// skips identity-less slots), and holds no hit (hits enumerate the live
    /// libraries). A label demand on a dead slot is a caller bug and
    /// surfaces as one.
    #[must_use]
    pub(crate) fn label(&self, library: u32) -> &str {
        self.idents[library as usize]
            .as_deref()
            .expect("live library")
    }

    /// The joined test graph (the `AffectedTests` join's read side).
    #[must_use]
    pub(crate) fn tests(&self) -> &TestGraph {
        &self.tests
    }
}

/// The folded surface of one library over its member files: blake3 over the
/// member content hashes, the blake3-128 fold of the member API signatures,
/// the library-face declaration names with their member-level signature
/// digests (#1561). Per member the persisted hint decides the read face: a
/// hit `materialize`s the payload through the #341 load faces; a miss reads
/// the engine's resident facts through the #341 accessor face — analysis
/// has already computed them, so nothing is re-derived (parse-surface
/// diagnostics play no role: this face produces no diagnostics of its own).
///
/// The equivalence the fold stands on: both arms answer for the same
/// resident bytes — the content identity is the same blake3 over the same
/// bytes, the face sets coincide (the materialized element model and the
/// engine's residency are equal for equal content), the signature columns
/// are the same baked digest, and the member digests render from equal
/// element models — so a generation whose entries match the workspace folds
/// into the seed's exact snapshot. The `restored` table takes the payload's
/// file references (directive targets, diagnostic files); the surface face
/// reads none of them, but `materialize` mints ids through it by contract.
fn library_surface(
    engine: &Engine,
    members: &[FileId],
    strings: &StringTable,
    persisted: PersistedFactsHint<'_>,
    restored: &mut FileTable,
) -> FileSnapshot {
    let mut member_faces: Vec<(StringId, [u8; 16])> = Vec::new();
    let mut folded = Vec::with_capacity(members.len() * 32);
    let mut signatures = Vec::with_capacity(members.len());
    // The builtin diagnostic-code registry, built once per load (the #428
    // contract): `materialize` consults it per restored diagnostic, and a
    // per-member rebuild would reconstruct the ~270-entry map for every
    // snapshot hit.
    let codes = CodeRegistry::builtin();
    for &file in members {
        let bytes = engine.content(file).expect("resident member");
        // The content identity rides the query contract's 32-byte blake3
        // over the resident bytes — the same digest either arm answers for;
        // the payload's 16-byte column is the entry key's domain, not the
        // fold's.
        folded.extend_from_slice(&content_hash(bytes));
        // The materialize identity is the house 16-byte content id — the
        // same digest residency carries.
        let content = dartq_source::content_hash(bytes);
        let text = std::str::from_utf8(bytes).expect("engine content is UTF-8");
        let signature = if let Some(payload) = persisted(text) {
            // The signature column off the payload bytes before the
            // materializing move (`materialize` consumes the payload).
            let signature = payload.api_signature;
            let facts = payload.materialize(text, content, strings, &codes, &mut |path| {
                restored.intern(path)
            });
            member_faces.extend(
                facts
                    .elements
                    .member_digests(strings)
                    .into_iter()
                    .map(|(name, digest)| (name, *digest.as_bytes())),
            );
            signature
        } else {
            // The engine's residency is the authority here: the
            // analysis prelude computed these facts, so a snapshot miss
            // costs one map read — no rescan, no re-intern (the names'
            // `StringId`s are native to the query's table — the
            // engine's own), and no language-version drift (the file's
            // settled feature set rode the analysis).
            let facts = engine.file_facts(file).expect("resident member facts");
            member_faces.extend(
                facts
                    .elements
                    .member_digests(strings)
                    .into_iter()
                    .map(|(name, digest)| (name, *digest.as_bytes())),
            );
            *facts.api_signature().as_bytes()
        };
        signatures.push(signature);
    }
    // Ascending by name (the query's merge-walk and binary-search
    // contract); a name declared twice in one library — the
    // duplicate-definition defect the capture logs — keeps its first
    // (name, digest) pair: deterministic, and the defect is that log's
    // problem, not this face's.
    member_faces.sort_unstable();
    member_faces.dedup_by(|a, b| a.0 == b.0);
    FileSnapshot {
        content_hash: content_hash(&folded),
        api_signature: query::fold_api_signatures(&signatures),
        member_names: member_faces.iter().map(|(name, _)| *name).collect(),
        member_digests: member_faces.into_iter().map(|(_, digest)| digest).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::{open_engine, test_fixture::workspace};
    use dartq_index::{DeclKind, capture};
    use dartq_language::{DEFAULT_LANGUAGE_VERSION, FeatureSet};
    use dartq_parser::ParserFeatures;
    use dartq_scanner::{ScanConfig, scan};
    use rustc_hash::FxHashMap;
    use serde_json::json;
    use std::fs;

    /// A retired library slot is invisible to the report (#558): removing a
    /// file retires its library at the next apply, so the arena's slot
    /// count rises above the live count — the substrate's label table is
    /// arena-sized, and the dead slot holds no edges, no test identity, and
    /// no hit. Under the retired live-count sizing this test panicked
    /// (index out of bounds in `TestGraph::build`, the first consumer that
    /// walks the full node space).
    #[test]
    fn retired_library_slots_are_invisible_to_the_report() {
        let root = workspace(
            "impact_retired",
            &[
                ("lib/a.dart", "class A {}\n"),
                ("lib/b.dart", "import 'a.dart';\nclass B {}\n"),
                ("test/a_test.dart", "import 'a.dart';\nvoid main() {}\n"),
            ],
        );
        let (mut engine, _) = open_engine(&root, Some(&root.join("sdk"))).expect("engine");
        let target = root.join("lib/a.dart");
        let baseline = impact(&mut engine, &target, &[]).expect("seeded impact");
        assert!(
            baseline
                .affected
                .iter()
                .any(|row| row.path.ends_with("b.dart")),
            "b imports a: {:?}",
            baseline.affected
        );

        // The removal retires b's library when `impact`'s prelude applies:
        // the disk file must leave FIRST — the prelude drives residency
        // from discovery, and a still-present file is re-settled (the
        // pending overlay is keyed by file, so a discovery Set replaces
        // the Remove and the library never retires — the #558 fixture
        // defect). With the file gone, the pending Remove survives the
        // apply.
        std::fs::remove_file(root.join("lib/b.dart")).expect("delete b.dart");
        engine
            .remove_file(&root.join("lib/b.dart"))
            .expect("remove");
        let after = impact(&mut engine, &target, &[]).expect("post-retirement impact");
        // The state guard: the retirement must actually have happened —
        // live libraries below the arena's slot count is the exact shape
        // the substrate's arena sizing exists for. Without this, a silent
        // no-op retirement passes the row assertions vacuously.
        assert_eq!(
            engine.libraries().len(),
            2,
            "a and the test library stay live"
        );
        assert_eq!(
            engine.library_import_graph().nodes(),
            3,
            "b's arena slot stays retired-but-counted"
        );
        assert!(
            !after
                .affected
                .iter()
                .any(|row| row.path.ends_with("b.dart")),
            "the retired library is gone: {:?}",
            after.affected
        );
        assert!(
            after
                .affected_tests
                .iter()
                .any(|row| row.path.ends_with("a_test.dart")),
            "the live test edge survives: {:?}",
            after.affected_tests
        );

        std::fs::remove_dir_all(&root).ok();
    }

    /// The report's path columns spell through the one file locator: the
    /// changed symbols' `label|name` label half, the affected rows, and
    /// the test rows are root-relative inside the analysis root, and the
    /// envelope carries the root exactly once. (Files outside the root
    /// keep the absolute spelling — the foreign-package shape the query
    /// face's locator test pins.)
    #[test]
    fn report_columns_spell_through_the_locator() {
        let root = workspace(
            "impact_locator",
            &[
                ("lib/a.dart", "class A {}\n"),
                ("lib/b.dart", "import 'a.dart';\nclass B {}\n"),
                ("test/a_test.dart", "import 'a.dart';\nvoid main() {}\n"),
            ],
        );
        let (mut engine, _) = open_engine(&root, Some(&root.join("sdk"))).expect("engine");
        let report = impact(&mut engine, &root.join("lib/a.dart"), &[]).expect("impact");
        let canonical = root.canonicalize().expect("root");

        assert_eq!(
            report.root.as_deref(),
            Some(canonical.display().to_string().as_str()),
            "the locator root rides exactly once"
        );
        assert!(
            report
                .changed
                .iter()
                .all(|row| row.symbol.starts_with("lib/a.dart|")),
            "the sym label half is the located path: {:?}",
            report.changed
        );
        let paths: Vec<&str> = report
            .affected
            .iter()
            .map(|row| row.path.as_str())
            .collect();
        assert_eq!(paths, vec!["lib/b.dart"], "the affected row is located");
        let tests: Vec<&str> = report
            .affected_tests
            .iter()
            .map(|row| row.path.as_str())
            .collect();
        assert_eq!(tests, vec!["test/a_test.dart"], "the test row is located");
        dartq_schema::validate(
            &serde_json::to_string(&report).expect("serialize"),
            dartq_schema::Domain::Impact,
        )
        .expect("the born report is schema-valid");

        std::fs::remove_dir_all(&canonical).ok();
    }

    /// The `--impacted` selection's input shape: the workspace fixture with
    /// everything committed, then `lib/a.dart` modified — one unstaged
    /// change against `HEAD`.
    fn git_workspace(tag: &str, libraries: &[(&str, &str)]) -> PathBuf {
        let root = workspace(tag, libraries);
        let git = |args: &[&str]| {
            let ok = std::process::Command::new("git")
                .args(args)
                .current_dir(&root)
                .env("GIT_AUTHOR_NAME", "fixture")
                .env("GIT_AUTHOR_EMAIL", "fixture@example.com")
                .env("GIT_COMMITTER_NAME", "fixture")
                .env("GIT_COMMITTER_EMAIL", "fixture@example.com")
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .expect("git runs")
                .success();
            assert!(ok, "git {args:?} failed");
        };
        git(&["init", "-q"]);
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", "fixture"]);
        std::fs::write(root.join("lib/a.dart"), "class A2 {}\n").expect("modify a.dart");
        // The callers' root is canonical (`current_dir().canonicalize()`),
        // and the selection strips it off the engine's canonical paths —
        // on macOS the temp dir's `/var` spelling differs from its
        // canonical `/private/var` form.
        root.canonicalize().expect("root")
    }

    /// The selection is the semantic reach — the one answer every test face
    /// (the CLI run and the daemon's child) shares for the same input: with
    /// `lib/a.dart` changed since `HEAD`, the suite whose import reaches it
    /// is selected and the same-package suite that imports nothing of it
    /// stays out — the discrimination the retired same-package heuristic
    /// could not make.
    #[test]
    fn impacted_tests_select_the_semantic_reach() {
        let root = git_workspace(
            "impact_sel_reach",
            &[
                ("lib/a.dart", "class A {}\n"),
                ("lib/b.dart", "import 'a.dart';\nclass B {}\n"),
                (
                    "test/a_test.dart",
                    "import 'package:fixture/a.dart';\nvoid main() {}\n",
                ),
                ("test/other_test.dart", "void main() {}\n"),
            ],
        );
        let selected =
            impacted_tests(&root, Some(&root.join("sdk")), "HEAD").expect("bounded selection");
        assert_eq!(
            selected,
            vec!["test/a_test.dart".to_owned()],
            "the changed lib file's semantic reach alone"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// The no-repository boundary: nothing to diff against, so the selection
    /// refuses — the caller's full run — with the diagnostic naming the
    /// base.
    #[test]
    fn impacted_tests_without_a_repository_run_everything() {
        let root = workspace("impact_sel_norepo", &[("lib/a.dart", "class A {}\n")]);
        let reason = impacted_tests(&root, Some(&root.join("sdk")), "HEAD")
            .expect_err("the full-run fallback");
        assert_eq!(reason, "cannot diff against HEAD (no git repository?)");
        std::fs::remove_dir_all(&root).ok();
    }

    /// The graph-unavailable boundary: the change is bounded by a
    /// repository, but the workspace's semantic graph cannot assemble (here:
    /// an unusable SDK root) — the conservative full run, never a partial
    /// guess.
    #[test]
    fn impacted_tests_fall_back_when_the_graph_cannot_assemble() {
        let root = git_workspace("impact_sel_noengine", &[("lib/a.dart", "class A {}\n")]);
        let reason = impacted_tests(&root, Some(&root.join("no-such-sdk")), "HEAD")
            .expect_err("the full-run fallback");
        assert!(
            reason.starts_with("cannot assemble the workspace:"),
            "names the assembly failure: {reason}"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// The scan configuration of every non-version-gated capture face (the
    /// workspace default: triple shift and augmentations on) — the payload
    /// projection's capture pass.
    const SCAN_CONFIG: ScanConfig = ScanConfig {
        enable_triple_shift: true,
        enable_augmentations: true,
    };

    /// Projects one file's facts onto the persisted payload the saver
    /// writes. The engine's resident facts are crate-private, so the
    /// projection re-derives the face rows with the same capture pass
    /// analysis uses: every library-face row becomes an element column
    /// entry (containers vs top-level declarations, names on the per-entry
    /// string slice, kinds in the persisted spellings) — the shape the load
    /// arm's derivation consumes. The fixture simplifies the face contents
    /// (all-zero signature column, bare `Unknown` types, empty member
    /// lists): the member digests differ between this projection and
    /// residency, harmlessly — the one-shot posture's rows key on the
    /// face-name sets alone (every name is an addition), and the report
    /// bytes never see a digest.
    #[expect(clippy::cast_possible_truncation)] // span arithmetic narrows under the scanner u32 offset cap
    fn persisted_facts(text: &str, file: FileId, strings: &StringTable) -> PersistedFacts {
        let scanned = scan(text, SCAN_CONFIG, file);
        let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(
            DEFAULT_LANGUAGE_VERSION,
        ));
        let captured = capture(file, text, &scanned.tokens, features, strings);
        let mut slice: Vec<String> = Vec::new();
        let mut slots: FxHashMap<String, u32> = FxHashMap::default();
        let top_decl = |kind: &str| {
            json!({
                "kind": kind,
                "type_params": 0,
                "var_ty": "Unknown",
                "return_ty": "Unknown",
                "formals": null,
            })
        };
        let container = |kind: &str| {
            json!({
                "kind": kind,
                "superclass": null,
                "members": [],
                "type_params": 0,
                "sealed": false,
                "is_abstract": false,
                "with_clause": [],
                "implements_clause": [],
                "on_clause": [],
                "header_residue": [],
                "primary_formals": [],
                "enum_constants": [],
                "constructors": [],
                "has_unnamed_constructor": true,
            })
        };
        let mut decls: Vec<serde_json::Value> = Vec::new();
        let mut containers: Vec<serde_json::Value> = Vec::new();
        let mut order: Vec<u32> = Vec::new();
        for row in captured.index.decls() {
            if !row.is_library_face() {
                continue;
            }
            let name = strings.resolve(row.name).to_owned();
            let at = if let Some(&at) = slots.get(&name) {
                at
            } else {
                slots.insert(name.clone(), slice.len() as u32);
                slice.push(name);
                (slice.len() - 1) as u32
            };
            match row.kind {
                DeclKind::Class => {
                    order.push(at);
                    containers.push(json!([at, container("Class")]));
                }
                DeclKind::Enum => {
                    order.push(at);
                    containers.push(json!([at, container("Enum")]));
                }
                DeclKind::Mixin => {
                    order.push(at);
                    containers.push(json!([at, container("Mixin")]));
                }
                DeclKind::Extension => {
                    order.push(at);
                    containers.push(json!([at, container("Extension")]));
                }
                DeclKind::ExtensionType => {
                    order.push(at);
                    containers.push(json!([at, container("ExtensionType")]));
                }
                DeclKind::TypeAlias => decls.push(json!([at, top_decl("TypeAlias")])),
                DeclKind::TopLevelFn => decls.push(json!([at, top_decl("TopLevelFn")])),
                DeclKind::TopLevelVar => decls.push(json!([at, top_decl("TopLevelVar")])),
                // Members and type parameters are never library faces; the
                // filter above already skipped them.
                _ => {}
            }
        }
        serde_json::from_value(json!({
            "api_signature": vec![0u8; 16],
            "directives": [],
            "doc_imports": [],
            "elements": {
                "strings": slice,
                "decls": decls,
                "containers": containers,
                "container_order": order,
                "extension_faces": [],
                "duplicates": [],
                "arity_violations": [],
            },
            "diagnostics": [],
            "files": [],
        }))
        .expect("saver-shape payload decodes")
    }

    /// The equivalence contract of the persisted surface: entries carried
    /// by the snapshot answer for their files through `materialize`, and
    /// the report over the loaded facts is byte-identical to the seeded
    /// baseline for the same content. The fixture spans every face kind
    /// plus a generic top-level function, whose type parameter must stay
    /// out of both derivations, and a part file folded into the seed
    /// library's surface.
    #[test]
    fn loaded_facts_reproduce_the_seeded_report() {
        let root = workspace(
            "impact_persisted",
            &[
                (
                    "lib/main.dart",
                    "part 'extra.dart';\n\n class Greeter { String hello() => 'hi'; }\n\
                     enum Color { red, green }\n mixin Runner {}\n\
                     extension GreeterRun on Greeter { void wave() {} }\n\
                     extension type Meters(int value) {}\n typedef Name = String;\n\
                     int counter = 0;\n String get greeting => 'hi';\n\
                     set greeting(String value) {}\n void greet<T>(T thing) {}\n",
                ),
                (
                    "lib/extra.dart",
                    "part of 'main.dart';\n\n void helper(int x) {}\n double ratio = 1.0;\n",
                ),
                (
                    "lib/other.dart",
                    "import 'main.dart';\n\n void use() { Greeter().hello(); }\n",
                ),
                ("test/main_test.dart", "void main() {}\n"),
            ],
        );
        let (mut engine, _) = open_engine(&root, Some(&root.join("sdk"))).expect("engine");
        let target = root.join("lib/main.dart");
        let seeded = impact(&mut engine, &target, &[]).expect("seeded impact");

        // The save side: one payload per workspace file, keyed by the exact
        // content the store's entry key pins.
        let strings = StringTable::new();
        let mut persisted: FxHashMap<String, PersistedFacts> = FxHashMap::default();
        for relative in [
            "lib/main.dart",
            "lib/extra.dart",
            "lib/other.dart",
            "test/main_test.dart",
        ] {
            let path = root.join(relative);
            let text = fs::read_to_string(&path).expect("fixture file");
            let file = engine.file_table().try_file_id(&path).expect("discovered");
            persisted.insert(text.clone(), persisted_facts(&text, file, &strings));
        }

        let loaded = impact_after_analysis_with_persisted_facts(
            &engine,
            &target,
            &|text: &str| persisted.get(text).cloned(),
            &[],
        )
        .expect("loaded impact");

        // The seed members (main.dart and its part) both hit; the report
        // bytes are the seeded baseline's.
        assert_eq!(
            serde_json::to_string(&loaded).expect("serialize"),
            serde_json::to_string(&seeded).expect("serialize"),
            "the loaded report is the seeded report",
        );
        // The comparison is not vacuous: the hypothetical change lists the
        // library's face names, and the type parameter is in neither
        // derivation's set.
        let names: Vec<&str> = loaded
            .changed
            .iter()
            .map(|row| row.symbol.as_str())
            .collect();
        assert!(
            names.iter().any(|row| row.ends_with("|Greeter")),
            "face rows: {names:?}"
        );
        assert!(
            names.iter().any(|row| row.ends_with("|greeting=")),
            "the setter's lookup spelling rides: {names:?}"
        );
        assert!(
            names.iter().any(|row| row.ends_with("|helper")),
            "the part file folds into the surface: {names:?}"
        );
        assert!(
            !names.iter().any(|row| row.ends_with("|T")),
            "the type parameter is not a library face: {names:?}"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    /// The fallback tier: a member the snapshot does not carry seeds exactly
    /// as before. With no hint at all — and with entries present for
    /// non-member files only — the report stays byte-identical to the
    /// seed: the per-entry decision consults the seed library's members
    /// alone, and a miss is a recompute, never an error.
    #[test]
    fn absent_entries_fall_back_to_seeding_per_member() {
        let root = workspace(
            "impact_absent",
            &[
                (
                    "lib/main.dart",
                    "part 'extra.dart';\n\n class Greeter { String hello() => 'hi'; }\n\
                     void greet<T>(T thing) {}\n",
                ),
                (
                    "lib/extra.dart",
                    "part of 'main.dart';\n\n void helper(int x) {}\n",
                ),
                (
                    "lib/other.dart",
                    "import 'main.dart';\n\n void use() { Greeter().hello(); }\n",
                ),
                ("test/main_test.dart", "void main() {}\n"),
            ],
        );
        let (mut engine, _) = open_engine(&root, Some(&root.join("sdk"))).expect("engine");
        let target = root.join("lib/main.dart");
        let seeded = impact(&mut engine, &target, &[]).expect("seeded impact");

        let hintless = impact_after_analysis(&engine, &target).expect("hintless impact");
        assert_eq!(
            serde_json::to_string(&hintless).expect("serialize"),
            serde_json::to_string(&seeded).expect("serialize"),
            "the hint-less posture is the seed",
        );

        // Entries exist — but only for files outside the seed library.
        let strings = StringTable::new();
        let mut persisted: FxHashMap<String, PersistedFacts> = FxHashMap::default();
        for relative in ["lib/other.dart", "test/main_test.dart"] {
            let path = root.join(relative);
            let text = fs::read_to_string(&path).expect("fixture file");
            let file = engine.file_table().try_file_id(&path).expect("discovered");
            persisted.insert(text.clone(), persisted_facts(&text, file, &strings));
        }
        let loaded = impact_after_analysis_with_persisted_facts(
            &engine,
            &target,
            &|text: &str| persisted.get(text).cloned(),
            &[],
        )
        .expect("partially loaded impact");
        assert_eq!(
            serde_json::to_string(&loaded).expect("serialize"),
            serde_json::to_string(&seeded).expect("serialize"),
            "missing seed members seed; foreign entries never perturb",
        );

        std::fs::remove_dir_all(&root).ok();
    }

    /// The naming-fallback precision (ai-audit X1) end-to-end over the real
    /// engine: two same-basename `a.dart` in different directories plus a
    /// sibling `test/a_test.dart` with no imports. The sibling seed sees
    /// its test; the homonym seed gains nothing — the old workspace-wide
    /// basename fan-out would have listed the test for both.
    #[test]
    fn test_fallback_resolves_the_sibling_only() {
        let root = workspace(
            "impact_sibling",
            &[
                ("lib/a.dart", "class A {}\n"),
                ("lib/other/a.dart", "class B {}\n"),
                ("test/a.dart", "class C {}\n"),
                ("test/a_test.dart", "void main() {}\n"),
            ],
        );
        let (mut engine, _) = open_engine(&root, Some(&root.join("sdk"))).expect("engine");

        let sibling = root.join("test/a.dart");
        let report = impact(&mut engine, &sibling, &[]).expect("sibling impact");
        let tests: Vec<&str> = report
            .affected_tests
            .iter()
            .map(|row| row.path.as_str())
            .collect();
        assert_eq!(tests.len(), 1, "the sibling test, and nothing else");
        assert!(tests[0].ends_with("test/a_test.dart"));

        let homonym = root.join("lib/other/a.dart");
        let report = impact(&mut engine, &homonym, &[]).expect("homonym impact");
        assert!(
            report.affected_tests.is_empty(),
            "no test edge reaches a same-basename homonym"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    /// The context fixture (the config tests' twin, plus an importer): a
    /// real package directory with two stub SDKs — `new`'s
    /// `allowed_experiments.json` grants package `fixture` the `variance`
    /// experiment, `old`'s does not, so `a.dart`'s `<in T>` modifier is the
    /// ERROR-severity discriminator between the axes (the `dart:` library
    /// table is the generated oracle-pinned flat face — axis-independent —
    /// so the per-package experiment whitelist is the surviving per-axis
    /// lever; `variance` has no `experimentalReleaseVersion`, so the grant
    /// applies only at the current language version, pinned to 3.13 by
    /// `package_config.json`) — and `b.dart` importing `a.dart`, so the
    /// affected set spans two libraries.
    fn context_fixture(tag: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("dq_cmd_impact_ctx_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let write = |relative: &str, content: &str| {
            std::fs::create_dir_all(root.join(relative).parent().expect("parent")).expect("dir");
            std::fs::write(root.join(relative), content).expect("file");
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
            r#"{"configVersion": 2, "packages": [{"name": "fixture", "rootUri": "../", "languageVersion": "3.13"}], "generator": "pub"}"#,
        );
        write("sdks/new/version", "3.13.1\n");
        write(
            "sdks/new/lib/libraries.json",
            r#"{"vm": {"libraries": {"core": {"uri": "core/core.dart"}, "async": {"uri": "async/async.dart"}}}}"#,
        );
        write("sdks/new/lib/async/async.dart", "library dart.async;\n");
        write("sdks/old/version", "3.5.0\n");
        write(
            "sdks/old/lib/libraries.json",
            r#"{"vm": {"libraries": {"core": {"uri": "core/core.dart"}}}}"#,
        );
        write(
            "sdks/new/lib/_internal/allowed_experiments.json",
            r#"{"version": 1, "experimentSets": {"grants": ["variance"]}, "sdk": {"default": {"experimentSet": "grants"}}, "packages": {"fixture": {"experimentSet": "grants"}}}"#,
        );
        write(
            "sdks/old/lib/_internal/allowed_experiments.json",
            r#"{"version": 1, "experimentSets": {"sdkExperiments": []}, "sdk": {"default": {"experimentSet": "sdkExperiments"}}, "packages": {}}"#,
        );
        write("lib/a.dart", "class Foo<in T> {}\n");
        write("lib/b.dart", "import 'a.dart';\nclass B {}\n");
        root
    }

    fn write_config(root: &Path, yaml: &str) {
        fs::write(root.join("dartq.yaml"), yaml).expect("config");
    }

    /// The declared contexts are the matrix, in file order, each judged by
    /// the compat verdict rule over the affected set: `a.dart`'s
    /// `variance`-gated modifier is clean under `new` (which grants the
    /// experiment) and an ERROR under `old`, so the seed-and-importer set
    /// flips exactly the `old` row. The primary sections ride unchanged.
    #[test]
    fn declared_contexts_drive_the_environment_matrix() {
        let root = context_fixture("matrix");
        write_config(
            &root,
            "version: 1\ncontexts:\n  new:\n    sdk: path:sdks/new\n  old:\n    sdk: path:sdks/old\n",
        );
        let (mut engine, _) = open_engine(&root, Some(&root.join("sdks/new"))).expect("engine");
        let target = root.join("lib/a.dart");
        let report = impact(&mut engine, &target, &[]).expect("impact");
        assert_eq!(
            report.affected_environments,
            vec![
                query::EnvironmentImpact {
                    label: "new".to_owned(),
                    compatible: true,
                },
                query::EnvironmentImpact {
                    label: "old".to_owned(),
                    compatible: false,
                },
            ],
            "one honest row per context, in file order"
        );
        assert!(
            report
                .affected
                .iter()
                .any(|row| row.path.ends_with("b.dart")),
            "the primary sections ride unchanged: {:?}",
            report.affected
        );

        std::fs::remove_dir_all(&root).ok();
    }

    /// The contextless degradation: no `dartq.yaml` anywhere up the chain —
    /// the single `default` row, its verdict the compat rule over the main
    /// pass. The clean analysis stays compatible.
    #[test]
    fn contextless_workspaces_keep_the_default_row() {
        let root = context_fixture("default");
        let (mut engine, _) = open_engine(&root, Some(&root.join("sdks/new"))).expect("engine");
        let report = impact(&mut engine, &root.join("lib/a.dart"), &[]).expect("impact");
        assert_eq!(
            report.affected_environments,
            vec![query::EnvironmentImpact {
                label: "default".to_owned(),
                compatible: true,
            }],
        );

        std::fs::remove_dir_all(&root).ok();
    }

    /// The contextless verdict is derived, never an unevaluated
    /// affirmative: with the main engine on the `old` SDK, `a.dart`'s
    /// `variance`-gated modifier is an ERROR on the affected set (the seed
    /// is in it), and the default row answers `compatible: false` — the
    /// same verdict the compat face's axis answers for the same SDK.
    #[test]
    fn a_contextless_target_with_an_error_answers_incompatible() {
        let root = context_fixture("default_error");
        let (mut engine, _) = open_engine(&root, Some(&root.join("sdks/old"))).expect("engine");
        let target = root.join("lib/a.dart");
        let report = impact(&mut engine, &target, &[]).expect("impact");
        assert_eq!(
            report.affected_environments,
            vec![query::EnvironmentImpact {
                label: "default".to_owned(),
                compatible: false,
            }],
            "the seed's ERROR flips the contextless row",
        );

        // The compat face's answer for the same axis agrees: the rule is
        // one, the faces scope it differently (affected set vs workspace).
        let (compat_report, _) =
            crate::compat::compat(&root, &target, &[root.join("sdks/old")]).expect("compat run");
        assert_eq!(
            compat_report.matrix[0].compatible, false,
            "the compat axis verdict matches the default row"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    /// An SDK root that fails to load isolates as an incompatible row (the
    /// compat doctrine): the matrix still answers, the healthy context's
    /// honest verdict rides, and the report ships.
    #[test]
    fn an_unloadable_context_sdk_is_an_incompatible_row() {
        let root = context_fixture("ghost");
        write_config(
            &root,
            "contexts:\n  ghost:\n    sdk: path:sdks/missing\n  new:\n    sdk: path:sdks/new\n",
        );
        let (mut engine, _) = open_engine(&root, Some(&root.join("sdks/new"))).expect("engine");
        let report = impact(&mut engine, &root.join("lib/a.dart"), &[]).expect("impact");
        assert_eq!(
            report.affected_environments,
            vec![
                query::EnvironmentImpact {
                    label: "ghost".to_owned(),
                    compatible: false,
                },
                query::EnvironmentImpact {
                    label: "new".to_owned(),
                    compatible: true,
                },
            ],
        );

        std::fs::remove_dir_all(&root).ok();
    }

    /// The matrix judges the affected set, not the workspace: seeded at the
    /// importer `b.dart`, the closure is `{b}` alone, so `a.dart`'s
    /// `old`-SDK ERROR lands outside the set and both contexts stay
    /// compatible — the whole-workspace verdict is the compat face's job.
    #[test]
    fn the_matrix_judges_the_affected_set_not_the_workspace() {
        let root = context_fixture("scope");
        write_config(
            &root,
            "contexts:\n  new:\n    sdk: path:sdks/new\n  old:\n    sdk: path:sdks/old\n",
        );
        let (mut engine, _) = open_engine(&root, Some(&root.join("sdks/new"))).expect("engine");
        let report = impact(&mut engine, &root.join("lib/b.dart"), &[]).expect("impact");
        assert!(
            report.affected.is_empty(),
            "no importer of b: {:?}",
            report.affected
        );
        assert_eq!(
            report.affected_environments,
            vec![
                query::EnvironmentImpact {
                    label: "new".to_owned(),
                    compatible: true,
                },
                query::EnvironmentImpact {
                    label: "old".to_owned(),
                    compatible: true,
                },
            ],
            "a.dart's error is outside the affected set"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    /// An unresolvable locator refuses the run (the compat face's usage
    /// tier), with the structured message naming the context and defect.
    #[test]
    fn an_unresolvable_locator_refuses() {
        let root = context_fixture("refuse");
        write_config(&root, "contexts:\n  bad:\n    sdk: registry:foo\n");
        let (mut engine, _) = open_engine(&root, Some(&root.join("sdks/new"))).expect("engine");
        let error = impact(&mut engine, &root.join("lib/a.dart"), &[]).expect_err("refusal");
        assert!(
            matches!(error, ImpactError::Usage(_)),
            "usage tier: {error:?}"
        );
        assert!(
            error.to_string().contains("unsupported sdk locator"),
            "structured: {error}"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    /// The canonical fixture report (the shaper tests' input): every row
    /// shape in one document — both signature verdicts, both impact
    /// levels, the test section, the two-row environment matrix, and the
    /// locator envelope (`/w` is the root the relative paths spell
    /// against). `reference_lines` rides the v3 row: non-empty on the
    /// semantic hit (its file holds resolved uses of the change's names)
    /// and empty on the transitive-only row.
    fn golden_report() -> query::ImpactReport {
        query::ImpactReport {
            schema_version: query::IMPACT_SCHEMA_VERSION,
            root: Some("/w".to_owned()),
            changed: vec![
                query::ChangedSymbol {
                    symbol: "lib/a.dart|Foo".to_owned(),
                    signature_changed: true,
                },
                query::ChangedSymbol {
                    symbol: "lib/a.dart|foo".to_owned(),
                    signature_changed: false,
                },
            ],
            affected: vec![
                query::AffectedTarget {
                    path: "lib/b.dart".to_owned(),
                    impact: query::ImpactLevel::Semantic,
                    reference_lines: vec![4, 5],
                },
                query::AffectedTarget {
                    path: "tool/note.dart".to_owned(),
                    impact: query::ImpactLevel::ContentOnly,
                    reference_lines: vec![],
                },
            ],
            affected_tests: vec![query::AffectedTest {
                path: "test/a_test.dart".to_owned(),
            }],
            affected_environments: vec![
                query::EnvironmentImpact {
                    label: "default".to_owned(),
                    compatible: true,
                },
                query::EnvironmentImpact {
                    label: "old".to_owned(),
                    compatible: false,
                },
            ],
        }
    }

    /// The AI-face golden (`goldens/ai/impact_report.json`): the promoted
    /// report envelope, pinned against the live shaper. The comparison
    /// rides the schema validate-at-birth gate inside [`to_json`]; a
    /// mismatch is a regression, not a golden to edit.
    #[test]
    fn the_ai_golden_pins_the_report_shaper() {
        let golden = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../goldens/ai/impact_report.json");
        let expected: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(&golden).expect("the impact golden rides the repo"),
        )
        .expect("one JSON object");
        assert_eq!(to_json(&golden_report()), expected, "impact golden drift");
    }

    /// The tier exactness boundary at the producer gate: the report is
    /// tier-invariant (every field a locator or a verdict the published
    /// schema requires at every tier), so the same payload is exact at
    /// minimal, compact, and full — the contract the CLI's grammar-parity
    /// `--verbosity` stands on.
    #[test]
    fn the_report_is_schema_valid_at_every_tier() {
        let text = to_json(&golden_report()).to_string();
        for tier in [
            dartq_schema::Verbosity::Minimal,
            dartq_schema::Verbosity::Compact,
            dartq_schema::Verbosity::Full,
        ] {
            dartq_schema::validate_at_tier(&text, dartq_schema::Domain::Impact, tier)
                .expect("tier-invariant: exact at every tier");
        }
    }

    /// `--symbol` narrows the seed's member face to the resolved hits: a
    /// container token and its member token resolve alike to the
    /// container's face name (`MyClass` — the signature fold is
    /// per-container, so `MyClass.method` narrows nothing past it). The
    /// `changed` list loses the un-named face (`topFn`), the propagation
    /// set is the resolved hits only, and `reference_lines` collects the
    /// use-site lines of those hits.
    #[test]
    fn symbol_flag_narrows_the_seed_face_and_collects_reference_lines() {
        let root = workspace(
            "impact_symbol_narrow",
            &[
                (
                    "lib/a.dart",
                    "class MyClass {\n  void method() {}\n}\nint topFn() => 0;\n",
                ),
                (
                    "lib/b.dart",
                    // `MyClass` referenced on lines 3 and 5 (1-based);
                    // `topFn` is the import-only face the narrowed seed
                    // drops — no use site exists, so the filter cannot
                    // prove it either way.
                    "import 'a.dart';\n\nMyClass make() => MyClass();\n\nvoid use(MyClass c) { c.method(); }\n",
                ),
            ],
        );
        let (mut engine, _) = open_engine(&root, Some(&root.join("sdk"))).expect("engine");
        let target = root.join("lib/a.dart");

        // The container face alone narrows `changed` to `MyClass`: the
        // member token and the bare container token resolve alike.
        for symbols in [
            vec!["MyClass".to_owned()],
            vec!["MyClass.method".to_owned()],
        ] {
            let report = impact(&mut engine, &target, &symbols).expect("symbol impact");
            let names: Vec<&str> = report
                .changed
                .iter()
                .map(|row| row.symbol.as_str())
                .collect();
            assert_eq!(
                names.len(),
                1,
                "the narrowed seed ships one face: {names:?}"
            );
            assert!(
                names[0].ends_with("|MyClass"),
                "the container face: {names:?}"
            );

            // The reference-line column of the consumer row: the resolved
            // hits' use-site lines in b.dart's defining file. `MyClass` at
            // lines 3 (`=> MyClass()`) and 5 (`use(MyClass c)`); line 1's
            // import and the member's `c.method()` touch no library-face
            // name — the declaration-resolution layer attributes them to
            // the container or the member per RefRow's target_name.
            let b = report
                .affected
                .iter()
                .find(|row| row.path.ends_with("b.dart"))
                .expect("b.dart is affected");
            assert!(
                !b.reference_lines.is_empty(),
                "the consumer's use-site lines land: {:?}",
                b.reference_lines
            );
            assert!(
                b.reference_lines.iter().all(|line| *line >= 1),
                "1-based lines: {:?}",
                b.reference_lines
            );
        }
        fs::remove_dir_all(&root).ok();
    }

    /// The three `--symbol` spellings (#2621) over one fixture: a bare
    /// member name unions every container declaring it, `Container.name`
    /// narrows to that container (constructor selectors included), and
    /// `file:line` selects the declaration whose name token starts on the
    /// line — a member line resolving to its container's face. Every miss
    /// stays the usage tier.
    ///
    /// `lib/repo.dart` under test (the seed):
    ///
    /// ```text
    /// 1: abstract class TodoRepository {
    /// 2:   Future<void> createTask(String title);
    /// 3: }
    /// 4: class Other {
    /// 5:   Future<void> createTask(String title) {}
    /// 6:   Other.named();
    /// 7: }
    /// ```
    #[test]
    fn symbol_spellings_resolve_members_containers_and_lines() {
        let root = workspace(
            "impact_symbol_spellings",
            &[
                (
                    "lib/repo.dart",
                    "abstract class TodoRepository {\n  Future<void> createTask(String title);\n}\nclass Other {\n  Future<void> createTask(String title) {}\n  Other.named();\n}\n",
                ),
                (
                    "lib/client.dart",
                    "import 'repo.dart';\nclass Client {\n  void run() {}\n}\n",
                ),
            ],
        );
        let (mut engine, _) = open_engine(&root, Some(&root.join("sdk"))).expect("engine");
        let target = root.join("lib/repo.dart");
        let faces = |report: &query::ImpactReport| -> Vec<String> {
            let mut names: Vec<String> = report
                .changed
                .iter()
                .map(|row| row.symbol.clone())
                .collect();
            names.sort();
            names
        };

        // Bare `createTask`: the union — both containers declaring it.
        let report =
            impact(&mut engine, &target, &["createTask".to_owned()]).expect("bare member name");
        let names = faces(&report);
        assert_eq!(names.len(), 2, "both declaring containers seed: {names:?}");
        assert!(
            names
                .iter()
                .all(|row| { row.ends_with("|TodoRepository") || row.ends_with("|Other") })
        );

        // `Container.name`: the named container's face alone.
        let report = impact(
            &mut engine,
            &target,
            &["TodoRepository.createTask".to_owned()],
        )
        .expect("container-qualified spelling");
        let names = faces(&report);
        assert_eq!(names.len(), 1);
        assert!(names[0].ends_with("|TodoRepository"), "{names:?}");

        // A constructor selector: `Other.named` narrows to `Other`.
        let report = impact(&mut engine, &target, &["Other.named".to_owned()])
            .expect("constructor selector spelling");
        let names = faces(&report);
        assert_eq!(names.len(), 1);
        assert!(names[0].ends_with("|Other"), "{names:?}");

        // `file:line`: the container header (line 1), the member (line 2,
        // resolving to its container), and the second container (line 4).
        for (line, face) in [
            (1u32, "|TodoRepository"),
            (2, "|TodoRepository"),
            (4, "|Other"),
        ] {
            let spelling = format!("{}:{line}", target.display());
            let report = impact(&mut engine, &target, &[spelling]).expect("file:line seed");
            let names = faces(&report);
            assert_eq!(names.len(), 1, "line {line}: {names:?}");
            assert!(names[0].ends_with(face), "line {line}: {names:?}");
        }

        // Misses stay the usage tier: a member no container declares, a
        // container that declares nowhere, a line with no name token (the
        // closing brace), a file outside the seed library, and a line the
        // element face cannot name (the constructor at line 6 — capture
        // rows carry no constructors).
        for spelling in [
            "TodoRepository.absent".to_owned(),
            "NoSuchClass.createTask".to_owned(),
            format!("{}:7", target.display()),
            format!("{}:1", root.join("lib/client.dart").display()),
            format!("{}:6", target.display()),
        ] {
            let error = impact(&mut engine, &target, &[spelling.clone()])
                .expect_err("a miss is the usage tier");
            assert!(
                matches!(error, ImpactError::Usage(_)),
                "{spelling}: {error:?}"
            );
            assert!(
                error.to_string().contains(&spelling),
                "the spelling rides: {error}"
            );
        }

        fs::remove_dir_all(&root).ok();
    }

    /// Multiple `--symbol` flags union: the seed face carries the union's
    /// resolved hits in the canonical ascending order the query's merge
    /// walk produces, not the flag order.
    #[test]
    fn repeated_symbol_flags_union_in_canonical_order() {
        let root = workspace(
            "impact_symbol_union",
            &[
                (
                    "lib/a.dart",
                    // `B` declared before `A` on disk; the canonical order
                    // is the interned-name sort, never the declaration or
                    // flag order.
                    "class Zebra {}\nclass Apple {}\n",
                ),
                ("lib/b.dart", "import 'a.dart';\nvoid use() {}\n"),
            ],
        );
        let (mut engine, _) = open_engine(&root, Some(&root.join("sdk"))).expect("engine");
        let target = root.join("lib/a.dart");

        // The flag order is `Zebra, Apple`; the canonical name order is
        // `Apple, Zebra` — the changed list proves the union sorted.
        let report = impact(
            &mut engine,
            &target,
            &["Zebra".to_owned(), "Apple".to_owned()],
        )
        .expect("union impact");
        let names: Vec<&str> = report
            .changed
            .iter()
            .map(|row| row.symbol.as_str())
            .collect();
        assert_eq!(names.len(), 2, "the union carries both hits: {names:?}");
        assert!(
            names[0].ends_with("|Apple") && names[1].ends_with("|Zebra"),
            "canonical ascending order: {names:?}"
        );
        fs::remove_dir_all(&root).ok();
    }

    /// A `--symbol` that resolves to no face name of the seed library is a
    /// usage refusal — `ImpactError::Usage`, the CLI's exit-2 tier — never
    /// a silent file-level fallback.
    #[test]
    fn an_unresolvable_symbol_is_the_usage_tier() {
        let root = workspace("impact_symbol_miss", &[("lib/a.dart", "class A {}\n")]);
        let (mut engine, _) = open_engine(&root, Some(&root.join("sdk"))).expect("engine");
        let target = root.join("lib/a.dart");
        let error = impact(&mut engine, &target, &["Missing".to_owned()])
            .expect_err("a miss is the usage tier");
        match error {
            ImpactError::Usage(message) => {
                assert!(message.contains("Missing"), "the name rides: {message}");
            }
            other => panic!("expected Usage, got {other:?}"),
        }
        fs::remove_dir_all(&root).ok();
    }

    /// The file-level posture is backward compatible: no `--symbol` seeds
    /// the library's whole face — `changed` lists every face name, and
    /// `reference_lines` collects every propagation-set use.
    #[test]
    fn no_symbol_flag_is_the_file_level_seed() {
        let root = workspace(
            "impact_file_level",
            &[
                ("lib/a.dart", "class A {}\nint topFn() => 0;\n"),
                ("lib/b.dart", "import 'a.dart';\nA make() => A();\n"),
            ],
        );
        let (mut engine, _) = open_engine(&root, Some(&root.join("sdk"))).expect("engine");
        let target = root.join("lib/a.dart");
        let report = impact(&mut engine, &target, &[]).expect("file-level impact");
        let names: Vec<&str> = report
            .changed
            .iter()
            .map(|row| row.symbol.as_str())
            .collect();
        assert_eq!(names.len(), 2, "the whole face seeds: {names:?}");
        fs::remove_dir_all(&root).ok();
    }

    /// The `--symbol` posture is deterministic: two runs over the same
    /// fixture answer byte-identical reports (the seed-name set and the
    /// reference-line projection are both unordered-build → sorted-write).
    #[test]
    fn symbol_runs_are_byte_identical() {
        let root = workspace(
            "impact_symbol_determinism",
            &[
                ("lib/a.dart", "class A {}\nclass B {}\n"),
                (
                    "lib/b.dart",
                    "import 'a.dart';\nA a() => A();\nB b() => B();\n",
                ),
            ],
        );
        let (mut engine, _) = open_engine(&root, Some(&root.join("sdk"))).expect("engine");
        let target = root.join("lib/a.dart");
        let symbols = vec!["A".to_owned(), "B".to_owned()];
        let first = impact(&mut engine, &target, &symbols).expect("first run");
        let second = impact(&mut engine, &target, &symbols).expect("second run");
        assert_eq!(
            serde_json::to_string(&first).expect("first serializes"),
            serde_json::to_string(&second).expect("second serializes"),
            "the report is byte-identical across runs",
        );
        fs::remove_dir_all(&root).ok();
    }

    /// The reference-line projection's empty state: a consumer that
    /// imports the seed library but names no seed face in its source
    /// (transitive-only reach — the propagation verdict holds, the column
    /// is empty) ships `reference_lines: []`.
    #[test]
    fn transitive_only_reach_ships_empty_reference_lines() {
        let root = workspace(
            "impact_transitive_only",
            &[
                ("lib/a.dart", "class A {}\n"),
                ("lib/b.dart", "import 'a.dart';\nclass B {}\n"),
            ],
        );
        let (mut engine, _) = open_engine(&root, Some(&root.join("sdk"))).expect("engine");
        let target = root.join("lib/a.dart");
        let report = impact(&mut engine, &target, &[]).expect("impact");
        let b = report
            .affected
            .iter()
            .find(|row| row.path.ends_with("b.dart"))
            .expect("b.dart is affected");
        assert_eq!(
            b.reference_lines,
            Vec::<u32>::new(),
            "the import-only consumer names no face: {:?}",
            b.reference_lines
        );
        fs::remove_dir_all(&root).ok();
    }

    /// Writes the `sky_engine`-shaped package: `lib/_embedder.yaml` mapping
    /// `dart:core` and `dart:ui` inside the package, plus the mapped files.
    fn write_sky_engine(root: &Path) {
        fs::create_dir_all(root.join("sky_engine/lib/ui")).expect("package dirs");
        fs::write(
            root.join("sky_engine/lib/_embedder.yaml"),
            "embedded_libs:\n  \"dart:core\": \"embedder_core.dart\"\n  \"dart:ui\": \"ui/ui.dart\"\n",
        )
        .expect("embedder yaml");
        fs::write(
            root.join("sky_engine/lib/embedder_core.dart"),
            "library dart.core;",
        )
        .expect("embedder core");
        fs::write(root.join("sky_engine/lib/ui/ui.dart"), "library dart.ui;").expect("embedder ui");
    }

    /// The fixture's package config plus one `sky_engine` entry
    /// (config-relative `rootUri`, the pub-writer spelling).
    fn rewrite_config_with_sky_engine(root: &Path) {
        fs::write(
            root.join(".dart_tool/package_config.json"),
            r#"{"configVersion": 2, "packages": [{"name": "fixture", "rootUri": "../", "packageUri": "lib/", "languageVersion": "3.5"}, {"name": "sky_engine", "rootUri": "../sky_engine", "packageUri": "lib/", "languageVersion": "3.5"}], "generator": "pub", "generatorVersion": "3.13.1"}"#,
        )
        .expect("config");
    }

    /// The environment axis rides the resolution's embedder table (#2633):
    /// a `sky_engine` entry turns the axis engine's `dart:` table into the
    /// embedder's replacement, so an affected `import 'dart:ui'` file
    /// analyzes compatible; without the entry the same file's
    /// `uri_does_not_exist` ERROR flips the verdict. The affected set
    /// spells through the locator (the verdict rule's own projection).
    #[test]
    fn environment_verdict_rides_the_resolution_embedder_table() {
        let affected: FxHashSet<String> = ["lib/main.dart".to_owned()].into_iter().collect();

        // The embedder-backed resolution: the environment is compatible.
        let root = workspace("impact_embedder", &[("lib/main.dart", UI_IMPORT)]);
        write_sky_engine(&root);
        rewrite_config_with_sky_engine(&root);
        let root = root.canonicalize().expect("root");
        let (config, graph, config_dir, _) = package_assembly_strict(&root).expect("assembly");
        assert!(
            analyze_environment(&root.join("sdk"), &config, &graph, &config_dir, &affected)
                .expect("the embedder environment answers"),
            "dart:ui resolves through the embedder"
        );
        fs::remove_dir_all(&root).ok();

        // The plain resolution: the missing `dart:ui` ERROR flips it.
        let plain = workspace("impact_embedder_plain", &[("lib/main.dart", UI_IMPORT)]);
        let plain = plain.canonicalize().expect("root");
        let (config, graph, config_dir, _) = package_assembly_strict(&plain).expect("assembly");
        assert!(
            !analyze_environment(&plain.join("sdk"), &config, &graph, &config_dir, &affected)
                .expect("the plain environment answers"),
            "dart:ui does not exist on the plain table"
        );
        fs::remove_dir_all(&plain).ok();
    }

    /// The environment-axis target of the embedder test: one `dart:ui`
    /// import, clean of every other diagnostic face.
    const UI_IMPORT: &str = "import 'dart:ui';\nvoid main() {}\n";
}
