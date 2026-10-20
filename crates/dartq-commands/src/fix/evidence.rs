use super::*;


/// The engine sources' check pass: one throwaway engine over the
/// discovered targets (`sdk` assembles it; `None` falls back to the
/// `DART_SDK` environment), one discover + analyze run — no lint stage,
/// [`collect_set`] runs the lint face itself — and the semantic drop-family
/// and parser-face diagnostics grouped under the discovery spelling of
/// each target file. Files the engine's context never interned (targets
/// outside the assembled workspace) carry no rows.
///
/// The declaration-synthesis evidence face rides the same pass: the target
/// files whose rows carry a synthesis-arm code also answer
/// [`FileEvidence`] rows (built while the engine — the capture rows and
/// the per-file Analysis seam — is still alive; the rest of the targets
/// never pay the build). The answer's evidence vec is path-sorted, the
/// machine face's deterministic order.
///
/// The import-synthesis family rides the same pass a third way: its
/// candidates need the maintained index ([`dartq_engine::Engine::query_symbols`])
/// and the directive facts, both engine-resident, so the harvest runs
/// here into per-file [`ImportContext`]s the per-file loop consults —
/// zero extra analysis, zero re-scanning.
///
/// Best-effort by contract: an assembly or analysis failure prints one
/// stderr note and yields an empty map — the fix run answers from the
/// other sources, never fails on the engine faces' account.
pub(crate) fn semantic_fix_diagnostics(
    files: &std::collections::BTreeSet<PathBuf>,
    sdk: Option<&Path>,
    fuse: &PlannerFuse,
) -> (
    FxHashMap<PathBuf, Vec<Diagnostic>>,
    Vec<FileEvidence>,
    FxHashMap<PathBuf, ImportContext>,
) {
    let mut rows = FxHashMap::default();
    // The synthesis arm's codes are by definition outside the mechanical
    // table (the table never claims them), so they group separately — the
    // evidence face's input.
    let mut synthesis_rows: FxHashMap<PathBuf, Vec<Diagnostic>> = FxHashMap::default();
    // The import arm's rows group the same way — the harvest's per-file
    // input (the miss names the candidates resolve from).
    let mut import_rows: FxHashMap<PathBuf, Vec<Diagnostic>> = FxHashMap::default();
    let mut evidence = Vec::new();
    let mut imports: FxHashMap<PathBuf, ImportContext> = FxHashMap::default();
    let Some(seed) = files.iter().next().and_then(|first| first.parent()) else {
        return (rows, evidence, imports);
    };
    // The advisory-taking opener, not [`crate::workspace::open_engine`]:
    // the query-family opener disarms the semantic diagnoser (`semdiag:
    // None`), and this pass exists to carry the semantic families — the
    // #236 analyze contract the diagnoser fills. The advisory (stale or
    // missing config) is the assembly's stderr note below, not a row.
    let Ok((mut engine, _, _)) = crate::workspace::open_engine_with_advisory(seed, sdk) else {
        eprintln!("dartq: semantic fix pass unavailable: no usable workspace or SDK");
        return (rows, evidence, imports);
    };
    let run = match crate::analyze::run_overlaid(&mut engine, &[], &[]) {
        Ok(run) => run,
        Err(error) => {
            eprintln!("dartq: semantic fix pass unavailable: {error}");
            return (rows, evidence, imports);
        }
    };
    // The owned diagnostics out of the run — its layout borrows the
    // engine, and the target resolution below reads the engine again.
    let crate::analyze::AnalyzeRun { diagnostics, .. } = run;
    // Discovery spelling per interned target: the rows key the same
    // spellings the per-file loop walks.
    let mut targets: FxHashMap<dartq_source::FileId, PathBuf> = FxHashMap::default();
    for path in files {
        if let Some(file) = engine.file_table().try_file_id(path)
            && !targets.contains_key(&file)
        {
            targets.insert(file, path.clone());
        }
    }
    if targets.is_empty() {
        return (rows, evidence, imports);
    }
    'grouping: for diagnostic in diagnostics {
        if !fuse.step() {
            break 'grouping;
        }
        let Some(path) = targets.get(&diagnostic.file) else {
            continue;
        };
        if crate::fix_table::is_engine_fix_code(diagnostic.code.name)
            || switch_case_arm(diagnostic.code.name).is_some()
        {
            rows.entry(path.clone())
                .or_insert_with(Vec::new)
                .push(diagnostic.clone());
        }
        if crate::fix_table::synthesis_arm(diagnostic.code.name).is_some() {
            synthesis_rows
                .entry(path.clone())
                .or_insert_with(Vec::new)
                .push(diagnostic.clone());
        }
        if import_arm(diagnostic.code.name).is_some() {
            import_rows
                .entry(path.clone())
                .or_insert_with(Vec::new)
                .push(diagnostic);
        }
    }
    'evidence: for (file, path) in &targets {
        if !fuse.step() {
            break 'evidence;
        }
        let Some(file_rows) = synthesis_rows.get(path) else {
            continue;
        };
        let built = synthesis_evidence(&engine, path, *file, file_rows, fuse);
        if !built.is_empty() {
            evidence.push(FileEvidence {
                path: path.display().to_string(),
                rows: built,
            });
        }
    }
    evidence.sort_unstable_by(|a, b| a.path.cmp(&b.path));
    // The import-synthesis harvest — engine-alive, per target file: the
    // arm rows' miss names resolve to candidate libraries through the
    // maintained index and the SDK faces.
    'imports: for (file, path) in &targets {
        if !fuse.step() {
            break 'imports;
        }
        let Some(file_rows) = import_rows.get(path) else {
            continue;
        };
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        if let Some(context) = import_contexts(&engine, *file, file_rows, &text, fuse) {
            imports.insert(path.clone(), context);
        }
    }
    (rows, evidence, imports)
}

// ---------------------------------------------------------------------------
// Declaration-synthesis evidence face (the D2 evidence rows)
// ---------------------------------------------------------------------------

/// The miss shape one synthesis evidence row carries: the name row the
/// diagnoser recorded at the diagnostic's offset, carried verbatim (the
/// spelling, the reference shape, the qualified flag) — never re-derived
/// from the text or the message at fix time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceName {
    /// The miss's spelling.
    pub name: String,
    /// The recorded reference shape.
    pub use_kind: dartq_semantic::NameUse,
    /// Whether the reference was member- or prefix-qualified — the
    /// static-vs-instance posture a callable producer keys on.
    pub qualified: bool,
}

/// One declaration-synthesis evidence row: the facts a create-family
/// producer composes its edit from, keyed to the diagnostic that decided
/// the face. The diagnostic's code and span are the **decision of record**
/// — the diagnoser's `take_context_miss` routing already chose which code
/// a miss reports (the type-argument, creation, catch, and clause faces
/// replace the `undefined_class` fallthrough), and this face never
/// re-derives that choice: a re-routed code rides its own arm exactly as
/// emitted. The position facts come from the capture's positional columns
/// (#1635), the shape columns resolve through the container row and the
/// per-file Analysis at producer time (#1637's substrate; not re-encoded
/// here).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixEvidence {
    /// The decision-of-record code name (`undefined_class`,
    /// `non_type_as_type_argument`, ...).
    pub code: &'static str,
    /// The diagnostic's UTF-16 span, verbatim.
    pub range: core::ops::Range<u32>,
    /// The miss's taken name row. `None` for the container-keyed arms
    /// (override and constructor/field synthesis read no miss name).
    pub name: Option<EvidenceName>,
    /// The synthesis insertion point, byte offset: the enclosing
    /// container's `body_open` (member synthesis) or the enclosing
    /// compilation-unit member's `decl_end` (top-level synthesis — the
    /// upstream `enclosingMember.end` convention). [`DeclRow::UNKNOWN`]
    /// where the capture supplies no honest position — the producer drops
    /// the synthesis rather than insert at a fabricated offset.
    pub insertion: u32,
    /// The enclosing container's row index in the file's capture
    /// ([`DeclRow::NONE`] for a top-level synthesis target); the shape
    /// columns resolve through it and the per-file Analysis at producer
    /// time.
    pub container: u32,
}

/// One file's evidence set: the synthesis rows keyed by the engine pass's
/// diagnostics. Files without rows never enter a set — the mirror of
/// [`FileFixSet`].
#[derive(Debug, Clone)]
pub struct FileEvidence {
    /// The file's discovery spelling (the fix set's key convention).
    pub path: String,
    /// The rows in their total key order: offset, code, span length, then
    /// the insertion facts (the determinism audit's R3 total-order ruling).
    pub rows: Vec<FixEvidence>,
}

/// Builds one file's synthesis evidence rows over the engine pass's
/// diagnostics for that file. The position facts read the engine's
/// capture; the miss names take the per-file Analysis seam's recorded
/// rows (the resolved assist face's isolated-context posture — the
/// owning library's member files under-report, the established M1
/// stance). Any file the pass cannot serve (no capture, unreadable text)
/// answers no rows — the face's under-report posture, never a fabricated
/// target.
pub(crate) fn synthesis_evidence(
    engine: &dartq_engine::Engine,
    path: &Path,
    file: FileId,
    diagnostics: &[Diagnostic],
    fuse: &PlannerFuse,
) -> Vec<FixEvidence> {
    if !diagnostics
        .iter()
        .any(|d| crate::fix_table::synthesis_arm(d.code.name).is_some())
    {
        return Vec::new();
    }
    let Some(capture) = engine.file_capture(file) else {
        return Vec::new();
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let members = [file];
    let context = crate::lints::build_semantic_context(engine, &text, file, &members);
    let analysis = dartq_semantic::analyze_unit(
        &text,
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
    let mut names: Vec<dartq_semantic::NameRow> = analysis.names().to_vec();
    evidence_rows(
        &text,
        capture,
        &mut names,
        analysis.strings(),
        diagnostics,
        fuse,
    )
}

/// The evidence face's pure core: the rows over one file's text, capture,
/// and recorded miss rows. The arm dispatch keys on the diagnostic's code
/// (the decision of record); the miss rows take-once per offset. Each
/// row's build routes through the planner fuse's counted step.
pub(crate) fn evidence_rows(
    text: &str,
    capture: &FileCapture,
    names: &mut Vec<dartq_semantic::NameRow>,
    strings: &dartq_source::StringTable,
    diagnostics: &[Diagnostic],
    fuse: &PlannerFuse,
) -> Vec<FixEvidence> {
    let mut rows: Vec<FixEvidence> = diagnostics
        .iter()
        .filter_map(|d| {
            if !fuse.step() {
                return None;
            }
            let arm = crate::fix_table::synthesis_arm(d.code.name)?;
            let name = match arm {
                crate::fix_table::SynthesisArm::TypeName
                | crate::fix_table::SynthesisArm::Callable => take_name_row(names, d.range.offset)
                    .map(|row| EvidenceName {
                        name: strings.resolve(row.name).to_owned(),
                        use_kind: row.use_kind,
                        qualified: row.qualified,
                    }),
                crate::fix_table::SynthesisArm::Override
                | crate::fix_table::SynthesisArm::Container => None,
            };

            // A miss-keyed row without its recorded name is an incomplete
            // synthesis target — dropped, never fabricated around (the
            // under-report posture: no edit beats a wrong edit).
            if name.is_none()
                && matches!(
                    arm,
                    crate::fix_table::SynthesisArm::TypeName
                        | crate::fix_table::SynthesisArm::Callable
                )
            {
                return None;
            }
            let (insertion, container) = insertion_point(&text, &capture, d.range.offset, arm);
            Some(FixEvidence {
                code: d.code.name,
                range: d.range.offset..d.range.offset + d.range.length,
                name,
                insertion,
                container,
            })
        })
        .collect();
    // The total order (determinism audit R3): source position, then the
    // decision-of-record code, then the span length, then the remaining
    // position facts — two rows tying on (start, code) (two diagnostics
    // over one span) keep a key-imposed order instead of the unstable
    // sort's internal one. A `message` leg has no counterpart here: the
    // evidence row carries the code and span as its decision of record,
    // and the insertion/container legs totalize what spans cannot.
    rows.sort_unstable_by(|a, b| {
        a.range
            .start
            .cmp(&b.range.start)
            .then_with(|| a.code.cmp(b.code))
            .then_with(|| a.range.end.cmp(&b.range.end))
            .then_with(|| a.insertion.cmp(&b.insertion))
            .then_with(|| a.container.cmp(&b.container))
            .then_with(|| name_order(&a.name, &b.name))
    });
    rows
}

/// The evidence rows' final tiebreak over the taken miss names: `None`
/// (the container-keyed arms) sorts first, two names order by spelling,
/// then shape, then qualification — the comparator's last leg, reachable
/// only for rows identical on code, span, and insertion facts.
pub(crate) fn name_order(a: &Option<EvidenceName>, b: &Option<EvidenceName>) -> core::cmp::Ordering {
    match (a, b) {
        (None, None) => core::cmp::Ordering::Equal,
        (None, Some(_)) => core::cmp::Ordering::Less,
        (Some(_), None) => core::cmp::Ordering::Greater,
        (Some(a), Some(b)) => a
            .name
            .cmp(&b.name)
            .then_with(|| use_rank(a.use_kind).cmp(&use_rank(b.use_kind)))
            .then_with(|| a.qualified.cmp(&b.qualified)),
    }
}

/// The recorded reference shape's stable rank (the [`use_label`] order).
pub(crate) fn use_rank(use_kind: dartq_semantic::NameUse) -> u8 {
    match use_kind {
        dartq_semantic::NameUse::Read => 0,
        dartq_semantic::NameUse::Write => 1,
        dartq_semantic::NameUse::Invoke => 2,
        dartq_semantic::NameUse::Type => 3,
        dartq_semantic::NameUse::Annotation => 4,
        dartq_semantic::NameUse::NamedArgument => 5,
    }
}

/// Consumes the name row recorded at `offset` — the fix-side mirror of the
/// diagnoser's `take_context_miss`: the rows ascend with source order, a
/// binary hit removes exactly the matching reference, and a taken row can
/// never feed a second decision at the same offset (exactly one consumer
/// per recorded miss — the drift guard).
pub(crate) fn take_name_row(
    rows: &mut Vec<dartq_semantic::NameRow>,
    offset: u32,
) -> Option<dartq_semantic::NameRow> {
    let index = rows.partition_point(|row| row.range.offset < offset);
    let hit = *rows.get(index).filter(|row| row.range.offset == offset)?;
    rows.remove(index);
    Some(hit)
}

/// The synthesis insertion point for a diagnostic at UTF-16 `offset16`,
/// per the arm's producer shape: a callable miss inside a container body
/// inserts at that container's `body_open` (the container's row index
/// carried); every other insertion targets the tightest enclosing
/// compilation-unit member — its `decl_end` for a top-level synthesis
/// (type-name and library-callable arms, [`DeclRow::NONE`] carried) or its
/// `body_open` when the diagnostic anchors on that member itself (the
/// override and constructor arms synthesize into their own container, and
/// answer the unknown when no container covers them). No covering row — a
/// directive position, or a declaration the parse never closed — answers
/// the honest unknown.
pub(crate) fn insertion_point(
    text: &str,
    capture: &FileCapture,
    offset16: u32,
    arm: crate::fix_table::SynthesisArm,
) -> (u32, u32) {
    let at = utf16_offset_to_byte(text, offset16);
    let decls = capture.index.decls();
    let covers = |row: &DeclRow| {
        row.decl_end != DeclRow::UNKNOWN
            && utf16_offset_to_byte(text, row.range.offset) <= at
            && at < row.decl_end as usize
    };
    // The arm's container: for a callable miss, the one container body
    // holding the offset (container extents do not nest); for the
    // override and constructor arms, the diagnostic's own container — the
    // tightest covering unit member.
    let container = match arm {
        crate::fix_table::SynthesisArm::Callable => decls
            .iter()
            .enumerate()
            .find(|(_, row)| row.body_open != DeclRow::UNKNOWN && covers(row)),
        crate::fix_table::SynthesisArm::Override | crate::fix_table::SynthesisArm::Container => {
            decls
                .iter()
                .enumerate()
                .filter(|(_, row)| row.is_library_face() && covers(row))
                .max_by_key(|(_, row)| row.range.offset)
        }
        crate::fix_table::SynthesisArm::TypeName => None,
    };
    if let Some((index, row)) = container {
        if row.body_open != DeclRow::UNKNOWN {
            return (row.body_open, index as u32);
        }
        // The member-synthesis container exists but carries no honest body
        // position — keep its row identity, withhold the offset.
        return (DeclRow::UNKNOWN, index as u32);
    }
    // Without the arm's container: the callable arm falls through to the
    // top-level synthesis (a library callable beside the enclosing unit
    // member); the override and constructor arms have no honest target at
    // all — their producers synthesize into a container only.
    if matches!(
        arm,
        crate::fix_table::SynthesisArm::Override | crate::fix_table::SynthesisArm::Container
    ) {
        return (DeclRow::UNKNOWN, DeclRow::NONE);
    }
    // Top-level synthesis: the tightest covering unit member's end.
    let unit = decls
        .iter()
        .filter(|row| row.is_library_face() && covers(row))
        .max_by_key(|row| row.range.offset);
    match unit {
        Some(row) => (row.decl_end, DeclRow::NONE),
        None => (DeclRow::UNKNOWN, DeclRow::NONE),
    }
}

/// The machine document for one file's evidence rows — `file`,
/// `schema_version`, and the `evidence` rows in their emission order: the
/// decision-of-record code and span, the taken miss name (`name` omitted
/// for the container-keyed arms), and the byte insertion point (omitted
/// where the capture supplies no honest position — a consumer never sees
/// a fabricated offset). The container's row index stays on the struct:
/// it dereferences only against the capture, which the document does not
/// carry. Deterministic bytes: the serializer's map keys sort, the rows
/// arrive offset-ordered.
#[must_use]
pub fn evidence_json(file: &FileEvidence) -> serde_json::Value {
    let rows: Vec<serde_json::Value> = file
        .rows
        .iter()
        .map(|row| {
            let mut json = serde_json::json!({
                "code": row.code,
                "offset": row.range.start,
                "length": row.range.end - row.range.start,
            });
            let object = json.as_object_mut().expect("evidence row object");
            if let Some(name) = &row.name {
                object.insert(
                    "name".into(),
                    serde_json::json!({
                        "spelling": name.name,
                        "use": use_label(name.use_kind),
                        "qualified": name.qualified,
                    }),
                );
            }
            if row.insertion != DeclRow::UNKNOWN {
                object.insert("insertion".into(), row.insertion.into());
            }
            json
        })
        .collect();
    serde_json::json!({
        "file": file.path,
        "schema_version": SCHEMA_VERSION,
        "evidence": rows,
    })
}

/// The JSON label of a recorded reference shape.
pub(crate) fn use_label(use_kind: dartq_semantic::NameUse) -> &'static str {
    match use_kind {
        dartq_semantic::NameUse::Read => "read",
        dartq_semantic::NameUse::Write => "write",
        dartq_semantic::NameUse::Invoke => "invoke",
        dartq_semantic::NameUse::Type => "type",
        dartq_semantic::NameUse::Annotation => "annotation",
        dartq_semantic::NameUse::NamedArgument => "named_argument",
    }
}

/// The file's unit features: its containing package's language version
/// through the package table's attribution walk (the engine's
/// `language_version_for` chain), the default
/// version where no table or no package attributes the file. `table` is
/// `None` when the run's workspace assembly failed — every file then runs
/// at the default posture, the face's pre-wiring behavior. This is the
/// package-version fallback the lint face also uses for unsettled files
/// (#722): `collect_set` discovers and parses engine-free, so the
/// engine's settled `@dart`/grant face cannot reach here — a `@dart`-pinned
/// file parses at its package posture, not its pinned one.
pub(crate) fn unit_features(table: Option<&PackageTable>, path: &Path) -> Features {
    // The query joins the table's spelling universe before the walk: the
    // attribution is purely lexical over the table's canonicalized roots
    // (the build's one normalize rule), while this face's paths come raw
    // off discovery — a user-spelled alias (`/var` vs `/private/var`, a
    // symlinked root) would strand at the walk's bound, attribute to no
    // package, and silently run the file at the default posture.
    let version = table
        .and_then(|table| table.containing(&normalize_path(path)))
        .and_then(|identity| identity.language_version)
        .map_or(
            DEFAULT_LANGUAGE_VERSION,
            dartq_pub::LanguageVersion::into_inner,
        );
    features_for(version)
}
