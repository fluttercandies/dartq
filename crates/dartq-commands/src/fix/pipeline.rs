use super::*;


/// The fix source stage: validates the targets through discovery, then
/// parses each file once through the lints layer's production entry and
/// runs its governing-options lint set; every hit the mechanical
/// rewrites can materialize (and whose source slice verifies) becomes an
/// anchored edit, and every lint hit is a `request/fixes` location for
/// the plugin hosts (one request per hit per live host — the protocol's
/// only query granularity). One fix set per patched file — files without
/// rows never enter the set. Each file runs under its containing package's
/// language version (`unit_features`).
///
/// The semantic source rides the same stage: one engine check pass over
/// the discovered targets (`semantic_fix_diagnostics`, assembled with
/// `sdk` — the CLI passes none and falls back to the `DART_SDK`
/// environment, the daemon passes its session's resolved root) contributes
/// the drop-family rows, merged into each file's set. A pass that cannot
/// run degrades to no rows and the run answers from the other sources.
///
/// The lint set is per-subtree, the analyze face's resolution
/// ([`crate::lints::analyze_lints`]): each file runs under the options
/// entry its own ancestry governs ([`AnalysisOptions::find_for_dir`]),
/// loaded once per distinct options file and keyed once per distinct
/// directory. No options file in the ancestry is the official
/// options-less posture — the empty set, and the file skips the lint run
/// (the opt-in cost ruling) unless the semantic pass holds rows for it.
/// The governing options' `cannot-ignore` set feeds [`run`]'s unignorable
/// gate, the analyze face's resolution — a code listed there survives
/// every ignore comment exactly as it does in the engine pass.
///
/// # Errors
///
/// [`DiscoverError`] from target discovery.
pub fn collect_set(
    paths: &[PathBuf],
    hosts: &mut PluginHosts,
    sdk: Option<&Path>,
) -> Result<Vec<FileFixSet>, DiscoverError> {
    collect_set_with_evidence(paths, hosts, sdk).map(|(sets, _)| sets)
}

/// [`collect_set`] with the declaration-synthesis evidence face: the same
/// single engine pass also answers the per-file evidence rows the
/// create-family producers compose their edits from (see [`FileEvidence`]
/// — and [`crate::fix_table::synthesis_arm`] for the code set). The
/// patch-document consumers keep [`collect_set`]; the producer wave and
/// the evidence document face take the richer answer.
///
/// # Errors
///
/// [`DiscoverError`] from target discovery.
pub fn collect_set_with_evidence(
    paths: &[PathBuf],
    hosts: &mut PluginHosts,
    sdk: Option<&Path>,
) -> Result<(Vec<FileFixSet>, Vec<FileEvidence>), DiscoverError> {
    let files = discover::discover(paths)?;
    // The walk's global progress fuse (#1687-class backstop): every
    // planner loop family below routes through its counted step.
    let fuse = PlannerFuse::new(files.len());
    // One workspace assembly per run, seeded from the first discovered
    // file, best-effort: a target set with no pub context around it — the
    // face's historical shape — stays at the default-version posture
    // instead of failing. Neither degrade leg is silent: the assembly's
    // advisory (stale/missing/corrupt config) prints its one-line message
    // on stderr like the analyze face's INFO row, and an assembly that
    // cannot run at all logs at the same "unavailable" tier as the
    // semantic pass below — the rows' semantic source degrades either
    // way, never silently.
    let assembly = files
        .iter()
        .next()
        .and_then(|first| first.parent())
        .and_then(|dir| match crate::workspace::package_assembly(dir) {
            Ok(assembly) => Some(assembly),
            Err(error) => {
                eprintln!("dartq: fix workspace assembly unavailable: {error}");
                None
            }
        });
    // The options-include faces: the assembly's package resolution when it
    // succeeded; a failed assembly degrades to the empty resolution — a
    // `package:` include then contributes nothing (the official
    // unresolvable-include posture) while the options file's own rules
    // still load.
    let (table, packages, config_dir, advisory) = match assembly {
        Some((table, packages, config_dir, _, advisory)) => {
            (Some(table), packages, config_dir, advisory)
        }
        None => (
            None,
            Arc::new(PackageConfig {
                packages: Vec::new(),
                generator: None,
                generator_version: None,
                additional_properties: BTreeMap::new(),
            }),
            PathBuf::new(),
            None,
        ),
    };
    if let Some(advisory) = advisory {
        eprintln!("dartq: {}: {}", advisory.code().name, advisory.message());
    }
    // The semantic source's engine check pass, before the per-file loop;
    // the synthesis evidence face rides the same pass.
    let (semantic, evidence) = semantic_fix_diagnostics(&files, sdk, &fuse);
    // The ignore-diagnostic face's dedup sets (one line row per line and
    // code, one file row per code) and the per-options-file codes the
    // third producer groups into its own file sets after the walk.
    let mut offered_line: FxHashSet<(u32, &'static str)> = FxHashSet::default();
    let mut offered_file: FxHashSet<&'static str> = FxHashSet::default();
    let mut options_codes: FxHashMap<PathBuf, Vec<&'static str>> = FxHashMap::default();
    // The per-directory enable sets and cannot-ignore sets (governance is
    // by ancestry, so one directory always maps to one entry) and the
    // per-file loaded options (shared through the map, workspace members
    // under one repo-root options file parse it once) — the analyze face's
    // serial precompute, hoisted into the walk's cold path.
    let mut governed: FxHashMap<
        PathBuf,
        (EnableSet, rustc_hash::FxHashSet<String>, Option<PathBuf>),
    > = FxHashMap::default();
    let mut loaded: FxHashMap<PathBuf, Arc<AnalysisOptions>> = FxHashMap::default();
    let mut out = Vec::new();
    'files: for path in &files {
        if !fuse.step() {
            break 'files;
        }
        let Some(dir) = path.parent() else {
            continue;
        };
        let (set, unignorable, options_file) =
            governed.entry(dir.to_path_buf()).or_insert_with(|| {
                let options = AnalysisOptions::find_for_dir(dir).map(|file| {
                    let path = file.clone();
                    let options = loaded
                        .entry(file)
                        .or_insert_with(|| {
                            Arc::new(AnalysisOptions::load(&path, &packages, &config_dir))
                        })
                        .clone();
                    (path, options)
                });
                let (path, options) = match options {
                    Some((path, options)) => (Some(path), Some(options)),
                    None => (None, None),
                };
                let rules = options.as_ref().map(|options| options.lint_rules.clone());
                (
                    crate::lints::enable_set(rules.as_deref()),
                    options
                        .map(|options| options.unignorable().clone())
                        .unwrap_or_default(),
                    path,
                )
            });
        // map_or(&[], Vec::as_slice) breaks inference (E0271); the explicit
        // unwrap_or form is the compiling spelling.
        #[expect(clippy::map_unwrap_or)]
        let semantic_rows = semantic.get(path).map(Vec::as_slice).unwrap_or(&[]);
        if *set == EnableSet::none() && semantic_rows.is_empty() {
            continue;
        }
        // Span-capacity gates (#1543 bypass face): the parse builds its
        // `LineIndex` outside the engine's intake gates, so the stat
        // refuses before the read and the byte length is authoritative.
        // Either rejection skips the file with the analyze face's
        // rejection notice — the fix set loses the file, the run
        // continues over its siblings.
        if stat_capacity_error(path).is_some() {
            eprintln!("{}", rejection_notice(path));
            continue;
        }
        let Ok(text) = std::fs::read_to_string(path) else {
            // Unreadable or non-UTF-8 target: nothing to patch — the apply
            // face re-reads and reports on its own terms.
            continue;
        };
        if text.len() > MAX_SOURCE_BYTES {
            eprintln!("{}", rejection_notice(path));
            continue;
        }
        let parsed = parse_unit(&text, unit_features(table.as_ref(), path));
        let hits: Vec<Lint> = if *set == EnableSet::none() {
            Vec::new()
        } else {
            run(&parsed.unit(Some(path)), set, unignorable)
        };
        let mut applied = from_lints(&hits, &text, &parsed.lines, path, semantic_rows, &fuse);
        if fuse.tripped() {
            break 'files;
        }
        applied.extend(from_semantic(semantic_rows, &text, &parsed.lines, path, &fuse));
        applied.extend(from_parser(semantic_rows, &text, &parsed.lines, path, &fuse));
        if fuse.tripped() {
            break 'files;
        }
        // The ignore-diagnostic face: every diagnostic gets the line and
        // file offers (deduped; a `cannot-ignore` code gets none), and
        // its code groups into the governing options file's set.
        let mut offers: Vec<(&'static str, core::ops::Range<u32>)> = hits
            .iter()
            .map(|hit| (RULES[hit.rule.0 as usize].name, hit.range.clone()))
            .collect();
        offers.extend(
            semantic_rows
                .iter()
                .map(|d| (d.code.name, d.range.offset..d.range.offset + d.range.length)),
        );
        for (code, range) in offers {
            if !fuse.step() {
                break 'files;
            }
            if unignorable.contains(code) {
                continue;
            }
            let (line, _) = parsed.lines.line_col_utf16(&text, range.start);
            if offered_line.insert((line, code))
                && let Some(fix) = ignore_line_fix(code, range, &text, &parsed.lines)
            {
                applied.push(fix);
            }
            if offered_file.insert(code)
                && let Some(fix) = ignore_file_fix(code, &text, &parsed.lines)
            {
                applied.push(fix);
            }
            if let Some(file) = options_file.as_ref() {
                let codes = options_codes.entry(file.clone()).or_default();
                if !codes.contains(&code) {
                    codes.push(code);
                }
            }
        }
        let plugin = dispatch_fixes(hosts, path, &text, &parsed.lines, &hits, &fuse);
        if fuse.tripped() {
            break 'files;
        }
        if !applied.is_empty() || !plugin.is_empty() {
            out.push(FileFixSet {
                path: path.display().to_string(),
                fixes: applied,
                plugin,
            });
        }
    }
    // The analysis-options sets: one per governing options file, the
    // codes in a deterministic order, each row's edit anchored against
    // the options file's own text.
    let mut options: Vec<(PathBuf, Vec<&'static str>)> = options_codes.into_iter().collect();
    options.sort_unstable();
    'options: for (file, mut codes) in options {
        codes.sort_unstable();
        if !fuse.step() {
            break 'options;
        }
        let Ok(options_text) = std::fs::read_to_string(&file) else {
            continue;
        };
        if options_text.len() > MAX_SOURCE_BYTES {
            continue;
        }
        let lines = LineIndex::new(&options_text);
        let fixes: Vec<RuleFix> = codes
            .into_iter()
            .filter_map(|code| {
                if !fuse.step() {
                    return None;
                }
                let (offset, length, replacement) = ignore_options_edit(code, &options_text)?;
                Some(RuleFix {
                    rule: code,
                    message: format!("Ignore '{code}' in `analysis_options.yaml`"),
                    source: BUILTIN_IGNORE_SOURCE,
                    edit: anchored_edit(&options_text, &lines, offset, length, &replacement),
                    target: None,
                })
            })
            .collect();
        if !fixes.is_empty() {
            out.push(FileFixSet {
                path: file.display().to_string(),
                fixes,
                plugin: Vec::new(),
            });
        }
    }
    if fuse.tripped() {
        // The bounded structured trip record (never a panic): the run
        // answers its partial fix sets, the note names the spent budget —
        // a planner loop family stopped advancing and the walk cut it.
        eprintln!(
            "dartq: fix planner fuse tripped after {} operations; the run answers its partial fix sets",
            fuse.budget
        );
    }
    Ok((out, evidence))
}

/// One `request/fixes` per lint hit against the hosts. The request names
/// the file the way the plugin knows it — the absolute path, since the
/// engine's own diagnostic feed uses the file table's absolute paths.
pub(crate) fn dispatch_fixes(
    hosts: &mut PluginHosts,
    path: &Path,
    text: &str,
    lines: &LineIndex,
    hits: &[Lint],
    fuse: &PlannerFuse,
) -> Vec<PluginFix> {
    if hosts.is_empty() {
        return Vec::new();
    }
    let file = host_path(path);
    let mut rows = Vec::new();
    for hit in hits {
        if !fuse.step() {
            break;
        }
        rows.extend(hosts.fixes(
            &file,
            u64::from(hit.range.start),
            u64::from(hit.range.end - hit.range.start),
            text,
            lines,
        ));
    }
    rows
}

/// The path string a plugin request carries: absolute when the discovery
/// yielded a relative argument (the CLI resolves against the process cwd,
/// the daemon's faces already resolve against the workspace root).
pub(crate) fn host_path(path: &Path) -> String {
    if path.is_absolute() {
        return path.display().to_string();
    }
    let joined = std::env::current_dir().map_or_else(|_| path.to_path_buf(), |cwd| cwd.join(path));
    joined.display().to_string()
}

// ---------------------------------------------------------------------------
// Fix source: the dartq-lints layer's diagnostic rows
// ---------------------------------------------------------------------------

/// Maps the lints layer's rows to anchored edits — the table-driven
/// dispatch ([`crate::fix_table`]): every wired rule whose official fix is
/// a single-span mechanical rewrite has a [`crate::fix_table::FixSpec`]
/// row, and rules without one never produce an edit; a wired hit whose
/// slice no longer verifies is dropped rather than patched on faith.
/// Each hit's expansion routes through the planner fuse's counted step
/// and the walk stops consuming hits on a spent budget. The cross-file
/// rows ([`crate::fix_table::FixAction::CreateFile`]) address their
/// target instead (`RuleFix::target`), offered only while it is absent.
#[must_use]
pub(crate) fn from_lints(
    hits: &[Lint],
    text: &str,
    lines: &LineIndex,
    path: &Path,
    semantic: &[Diagnostic],
    fuse: &PlannerFuse,
) -> Vec<RuleFix> {
    // The structural rewriters verify against the token stream (keyword
    // positions are quote-safe only there); the parse is paid once per
    // call, and only when a hit's family needs it.
    let needs_tokens = hits.iter().any(|hit| {
        crate::fix_table::spec_for(RULES[hit.rule.0 as usize].name)
            .is_some_and(|spec| spec.source == FixSource::Lint && spec.needs_tokens)
    });
    let tokens = needs_tokens.then(|| parse_unit(text, Features::default()).tokens);
    let mut rows = Vec::new();
    for hit in hits {
        if !fuse.step() {
            break;
        }
        let rule = RULES[hit.rule.0 as usize].name;
        let Some(spec) = crate::fix_table::spec_for(rule) else {
            continue;
        };
        if spec.source != FixSource::Lint {
            continue;
        }
        let input = FixInput {
            text,
            range: hit.range.clone(),
            message: &hit.message,
            tokens: tokens.as_ref(),
            path,
            diagnostics: semantic,
        };
        if let Some((target, content)) = crate::fix_table::expand_create(spec, &input) {
            if !Path::new(&target).exists() {
                rows.push(created_file_row(
                    rule,
                    &hit.message,
                    BUILTIN_SOURCE,
                    target,
                    &content,
                ));
            }
            continue;
        }
        let Some(edits) = crate::fix_table::expand_multi(spec, &input) else {
            continue;
        };
        for (offset, length, replacement) in edits {
            rows.push(RuleFix {
                rule,
                message: hit.message.clone(),
                source: BUILTIN_SOURCE,
                edit: anchored_edit(text, lines, offset, length, &replacement),
                target: None,
            });
        }
    }
    rows
}

/// Maps the engine's semantic diagnostics to anchored edits — the fix
/// source of the drop family and the null-check family (module docs). The
/// table lookup ([`crate::fix_table`]) is flat: every row is a mechanical
/// rewrite through the same verify-then-anchor pipeline as [`from_lints`],
/// and a hit whose slice does not verify its shape drops rather than
/// patches. Diagnostics outside the two families never produce an edit.
///
/// The token-verified rows (the `unused_local_variable` removal variant
/// and the whole null-check family) pay one parse per call, and only when
/// one of their hits is present.
#[must_use]
pub(crate) fn from_semantic(
    diagnostics: &[Diagnostic],
    text: &str,
    lines: &LineIndex,
    path: &Path,
    fuse: &PlannerFuse,
) -> Vec<RuleFix> {
    engine_rows(
        diagnostics,
        text,
        lines,
        path,
        FixSource::Semantic,
        BUILTIN_SEMANTIC_SOURCE,
        fuse,
    )
}

/// Maps the engine's syntax diagnostics to anchored edits — the parser
/// source (module docs): the [`crate::fix_table`]'s parser rows mirror the
/// official registry's mechanical parser producers shape for shape, and a
/// report whose surroundings do not verify the producer's precondition
/// drops. The token-verified rows pay one parse per call, only when one
/// of their hits is present. Identical consecutive edits collapse: the
/// scanner reports a number literal's separators once per run, the
/// official producer rewrites the literal once.
#[must_use]
pub(crate) fn from_parser(
    diagnostics: &[Diagnostic],
    text: &str,
    lines: &LineIndex,
    path: &Path,
    fuse: &PlannerFuse,
) -> Vec<RuleFix> {
    let mut rows = engine_rows(
        diagnostics,
        text,
        lines,
        path,
        FixSource::Parser,
        BUILTIN_PARSER_SOURCE,
        fuse,
    );
    rows.dedup_by(|a, b| {
        a.edit.offset == b.edit.offset
            && a.edit.length == b.edit.length
            && a.edit.replacement == b.edit.replacement
    });
    rows
}

/// The engine-sourced executor behind [`from_semantic`] and
/// [`from_parser`]: the rows of `source` among `diagnostics`, expanded
/// through the table and anchored, tagged `provenance`. Each diagnostic's
/// expansion routes through the planner fuse's counted step and the walk
/// stops consuming diagnostics on a spent budget. The cross-file rows
/// ([`crate::fix_table::FixAction::CreateFile`]) address their target
/// instead (`RuleFix::target`), offered only while it is absent.
pub(crate) fn engine_rows(
    diagnostics: &[Diagnostic],
    text: &str,
    lines: &LineIndex,
    path: &Path,
    source: FixSource,
    provenance: &'static str,
    fuse: &PlannerFuse,
) -> Vec<RuleFix> {
    let needs_tokens = diagnostics.iter().any(|d| {
        crate::fix_table::spec_for(d.code.name)
            .is_some_and(|spec| spec.source == source && spec.needs_tokens)
    });
    let tokens = needs_tokens.then(|| parse_unit(text, Features::default()).tokens);
    let mut rows = Vec::new();
    for d in diagnostics {
        if !fuse.step() {
            break;
        }
        let Some(spec) = crate::fix_table::spec_for(d.code.name) else {
            continue;
        };
        if spec.source != source {
            continue;
        }
        let input = FixInput {
            text,
            range: d.range.offset..d.range.offset + d.range.length,
            message: &d.message,
            tokens: tokens.as_ref(),
            path,
            diagnostics,
        };
        if let Some((target, content)) = crate::fix_table::expand_create(spec, &input) {
            if !Path::new(&target).exists() {
                rows.push(created_file_row(
                    d.code.name,
                    &d.message,
                    provenance,
                    target,
                    &content,
                ));
            }
            continue;
        }
        let Some(edits) = crate::fix_table::expand_multi(spec, &input) else {
            continue;
        };
        for (offset, length, replacement) in edits {
            rows.push(RuleFix {
                rule: d.code.name,
                message: d.message.to_string(),
                source: provenance,
                edit: anchored_edit(text, lines, offset, length, &replacement),
                target: None,
            });
        }
    }
    rows
}

/// The cross-file row: the created file's content anchored at offset 0 of
/// its empty text, the row addressed to the target ([`RuleFix::target`])
/// — the shape the apply face materializes by writing the patched empty
/// buffer, which is the file coming into existence.
pub(crate) fn created_file_row(
    rule: &'static str,
    message: &str,
    provenance: &'static str,
    target: String,
    content: &str,
) -> RuleFix {
    let lines = LineIndex::new("");
    RuleFix {
        rule,
        message: message.to_owned(),
        source: provenance,
        edit: anchored_edit("", &lines, 0, 0, content),
        target: Some(target),
    }
}

/// Maps one plugin's `request/fixes` reply to anchored rows, priority
/// first (the reply's own order on ties — the sort is stable). Every edit
/// is bounds-checked against `text` before anchoring — plugin offsets are
/// untrusted wire data and [`anchored_edit`]'s converter panics on
/// out-of-range offsets — and one bad edit drops the whole change: a
/// proposal is applied atomically or not at all, never half-applied.
#[must_use]
pub fn from_plugin(
    plugin: &str,
    reply: &FixesReply,
    text: &str,
    lines: &LineIndex,
) -> Vec<PluginFix> {
    let len16 = utf16_len_of(text);
    let mut order: Vec<&PrioritizedChange> = reply.fixes.iter().collect();
    order.sort_by_key(|change| change.priority);
    order
        .into_iter()
        .filter_map(|change| {
            let mut edits = Vec::with_capacity(change.edits.len());
            for edit in &change.edits {
                let offset = u32::try_from(edit.offset).ok()?;
                let length = u32::try_from(edit.length).ok()?;
                if offset.checked_add(length)? > len16 {
                    return None;
                }
                edits.push(anchored_edit(
                    text,
                    lines,
                    offset,
                    length,
                    &edit.replacement,
                ));
            }
            Some(PluginFix {
                plugin: plugin.to_owned(),
                message: change.message.clone(),
                priority: change.priority,
                edits,
            })
        })
        .collect()
}

/// The resolved-light assist transport (the `collect_set` semantic-stage
/// posture): one throwaway workspace engine assembles the unit's
/// semantic fold ([`dartq_semantic::Analysis`]), the dispatch runs once
/// with the face mounted, and the engine is dropped. Any failure — no
/// workspace, the file outside the assembled engine, a fold the context
/// cannot build — degrades to the engine-free dispatch, never an error:
/// the resolved producers go silent and the rows answer from the
/// engine-free kinds. The face is the single-file posture (the owning
/// library's member files under-report to this file), the semantic
/// tier's established isolated-context stance — cross-file names
/// under-report `None` and the producers decline. `sdk` is the engine's
/// SDK root: the callers without one pass `None` and the assembly falls
/// back to `DART_SDK` discovery; the resolved tests pin the fixture SDK.
pub(crate) fn resolved_assist_rows(
    text: &str,
    path: &Path,
    offset: u64,
    length: u64,
    sdk: Option<&Path>,
    fuse: &PlannerFuse,
) -> Vec<PluginFix> {
    let Some(seed) = path.parent() else {
        return builtin_assists(text, path, offset, length, fuse);
    };
    let Ok((mut engine, _)) = crate::workspace::open_engine(seed, sdk) else {
        return builtin_assists(text, path, offset, length, fuse);
    };
    // The file table fills at discovery — without it the target file has
    // no id and the face degrades to the engine-free dispatch.
    let _ = engine.discover_workspace();
    let file_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let Some(file) = engine
        .file_table()
        .try_file_id(&file_path)
        .or_else(|| engine.file_table().try_file_id(path))
    else {
        return builtin_assists(text, path, offset, length, fuse);
    };
    let members = [file];
    let context = crate::lints::build_semantic_context(&engine, text, file, &members);
    let analysis = dartq_semantic::analyze_unit(
        text,
        &context.scanned.tokens,
        crate::lints::SEMANTIC_PARSE_FEATURES,
        engine.strings(),
        file,
        context.lib,
        &context.graph,
        &context.namespaces,
        &context.index,
        dartq_semantic::StrictModes::default(),
        &dartq_resolution::LibraryClassModifiers::default(),
    );
    builtin_assists_with(text, path, offset, length, Some(&analysis), fuse)
}

/// The location-directed assist face: the built-in producers' proposals
/// for the UTF-16 span `[offset, offset + length)` followed by the plugin
/// hosts' proposals for one file, all anchored against the file's on-disk
/// text. The built-in rows lead (priority order), the plugin rows follow
/// (the hosts' own priority order); the built-in parse is paid once per
/// request.
///
/// `sdk` backs the resolved-light assist mount's engine assembly — the
/// CLI passes none and falls back to the `DART_SDK` environment, the
/// daemon passes its session's resolved root.
///
/// # Errors
///
/// [`DiscoverError::Missing`] when the target does not exist (the usage
/// tier, the fix face's message body), [`DiscoverError::Io`] for any
/// other read failure, including the span-capacity rejection (#1543
/// bypass face: the message carries the path and both sizes).
pub fn assists(
    path: &Path,
    offset: u64,
    length: u64,
    hosts: &mut PluginHosts,
    sdk: Option<&Path>,
) -> Result<AssistAnswer, DiscoverError> {
    // Span-capacity gates: the stat refuses before the read, the byte
    // length is authoritative — the text never reaches `LineIndex::new`
    // over the capacity bound.
    if let Some(error) = stat_capacity_error(path) {
        return Err(DiscoverError::Io(error));
    }
    let text = std::fs::read_to_string(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            DiscoverError::Missing(path.display().to_string())
        } else {
            DiscoverError::Io(error)
        }
    })?;
    if text.len() > MAX_SOURCE_BYTES {
        return Err(DiscoverError::Io(capacity_error(path, text.len() as u64)));
    }
    let lines = LineIndex::new(&text);
    // The assist walk's planner fuse (#1687-class backstop): one request
    // plans one file, and every trial routes through the counted step. A
    // spent budget answers the partial rows with a structured error row —
    // the client sees the trip in `errors`, never a hang, never a panic.
    let fuse = PlannerFuse::new(1);
    let mut assists = resolved_assist_rows(&text, path, offset, length, sdk, &fuse);
    assists.extend(hosts.fixes(&host_path(path), offset, length, &text, &lines));
    let mut errors = hosts.take_errors();
    if fuse.tripped() {
        errors.push(PluginHostError {
            plugin: BUILTIN_ASSIST_SOURCE.to_owned(),
            message: format!(
                "the assist planner fuse tripped after {} operations; the answer carries the partial rows",
                fuse.budget
            ),
        });
    }
    Ok(AssistAnswer {
        file: path.display().to_string(),
        assists,
        errors,
    })
}
