//! `apply_pending`: the ContentId gate → facts recompute → directive diff →
//! library reformation → reverse-edge closure (incremental-l1-spec §3.2
//! steps 1–5).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use dartq_diagnostics::{CodeInfo, Diagnostic, DiagnosticType, Severity};
use dartq_language::{
    DEFAULT_LANGUAGE_VERSION, FeatureSet, LanguageVersion, triple_shift_enabled,
};
use dartq_parser::{Message, Parser, ParserFeatures, ParserListener, TokenSpan};
use dartq_resolution::{
    CombinatorClause, Combinators, ResolvedTarget, UriResolution, UriResolver,
};
use dartq_scanner::{Keyword, KeywordStyle, ScanConfig, TokenKind, TokenTable, scan};
use dartq_source::{ContentId, FileId, SourceRange, StringTable, utf16_len_of};
use rayon::prelude::*;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::event::{EngineEvent, RevalidateGate};
use crate::state::{
    DirectiveFacts, DirectiveKind, EngineState, FACTS_SEMANTIC_VERSION, FactsKey, LibraryId,
    LibraryInfo, PendingChange, UnitFacts,
};

/// The outcome of one apply (engine-api-spec §1.2 `AffectedReport` payload).
#[derive(Debug, Default)]
pub(crate) struct ApplyOutcome {
    pub changed: Vec<FileId>,
    pub removed: Vec<FileId>,
    pub invalidated: Vec<LibraryId>,
}

impl ApplyOutcome {
    fn empty() -> Self {
        Self::default()
    }
}

/// Drains the pending overlay and runs the L1 cascade.
///
/// Event sequence for a non-empty queue: `OverlayApplied`, then
/// `FileRevalidated` per touched file (ascending `FileId`), then one
/// `CascadeInvalidated` (possibly empty). An empty queue emits nothing.
pub(crate) fn apply(
    state: &mut EngineState,
    on_event: &mut dyn FnMut(EngineEvent),
) -> ApplyOutcome {
    if state.pending.is_empty() {
        return ApplyOutcome::empty();
    }

    // Step 1: the ContentId gate (official defers the comparison to here,
    // never to set_content — a rewrite with identical bytes is a legal no-op).
    let order = std::mem::take(&mut state.pending_order);
    let mut pending = std::mem::take(&mut state.pending);
    let mut changed: Vec<(FileId, dartq_source::ContentId, String)> = Vec::new();
    let mut removed: Vec<FileId> = Vec::new();
    let mut gate_hits: Vec<FileId> = Vec::new();
    for file in order {
        match pending.remove(&file) {
            Some(PendingChange::Set(text)) => {
                let content = state.store.put(text.as_bytes());
                if state
                    .residency
                    .get(&file)
                    .is_some_and(|facts| facts.content == content)
                {
                    gate_hits.push(file);
                } else {
                    changed.push((file, content, text));
                }
            }
            Some(PendingChange::Remove) => {
                if state.residency.remove(&file).is_some() {
                    removed.push(file);
                }
            }
            None => {}
        }
    }
    debug_assert!(pending.is_empty(), "drained entries must all be consumed");
    let changed_files: Vec<FileId> = changed.iter().map(|(file, _, _)| *file).collect();

    on_event(EngineEvent::OverlayApplied {
        changed: changed_files.clone(),
        removed: removed.clone(),
    });

    // Steps 2–3: facts recompute for gate-passing files (parallel scan/parse/
    // collect, sequential URI resolution), then residency update.
    let recomputed = recompute_facts(state, &changed);
    let mut gates: Vec<(FileId, RevalidateGate)> = gate_hits
        .into_iter()
        .map(|file| (file, RevalidateGate::ContentIdHit))
        .collect();
    gates.extend(recomputed.iter().map(|(file, _facts, cache_hit)| {
        (
            *file,
            RevalidateGate::Parsed {
                ast_cache_hit: *cache_hit,
            },
        )
    }));
    for (file, facts, _) in &recomputed {
        state.residency.insert(*file, Arc::clone(facts));
    }
    gates.sort_unstable_by_key(|(file, _)| *file);
    for (file, gate) in &gates {
        on_event(EngineEvent::FileRevalidated {
            file: *file,
            gate: *gate,
        });
    }

    // Steps 4–5: library reformation, reverse-edge rebuild, dirty seeds,
    // importer closure. The closure output *replaces* the phase-2 work queue
    // (state.rs: "post-closure dirty set of the last apply; consumed by
    // analyze") — assignment, never a union. A cancelled analyze may leave
    // unfinished libraries queued, but every still-invalid leftover is
    // re-derived here: close() re-seeds from the result cache (no cached
    // result or changed identity ⇒ dirty), and between the cancel and this
    // close only the completed-slot insert writes results — by definition
    // excluding the leftovers. (A leftover whose cached result matches its
    // rebuilt identity is genuinely fresh, so dropping it is sound.)
    // Assignment preserves the invariant `state.invalidated ⊆ live()`;
    // a union would resurrect ids retired by an intervening remove, and
    // analyze unwraps every queued id's info.
    let invalidated = reform_and_close(state);
    state.invalidated = invalidated.clone();
    on_event(EngineEvent::CascadeInvalidated {
        libraries: invalidated.clone(),
    });

    state.structure_dirty = true;
    ApplyOutcome {
        changed: changed_files,
        removed,
        invalidated,
    }
}

/// Recomputes the facts of every gate-passing file: fan-out over the
/// pure per-file work (language version → scan → directive diet walk → parse
/// diagnostics → element collection), then a sequential URI-resolution pass
/// (it interns directive targets into the shared `FileTable`).
///
/// A file whose `(content, feature fingerprint)` key already sits in the
/// facts cache skips the fan-out entirely (`ast_cache_hit`). Changed files
/// parse **twice** at M1 — once for recovery diagnostics through the
/// engine's listener, once inside `dartq_semantic::collect`, which
/// hardcodes its own listener. The waste is bounded to changed files and is
/// on record as the reason #120/#121 should land a combined listener face.
fn recompute_facts(
    state: &mut EngineState,
    changed: &[(FileId, ContentId, String)],
) -> Vec<(FileId, Arc<UnitFacts>, bool)> {
    // Language version per file: the owning package's version (the engine's
    // single source of truth for file → package attribution).
    let versions: Vec<LanguageVersion> = changed
        .iter()
        .map(|(file, _, _)| language_version_for(state, *file))
        .collect();

    // Cache lookups up front (read-only); misses dispatch to the fan-out
    // below.
    let mut slots: Vec<Option<(Arc<UnitFacts>, bool)>> = Vec::with_capacity(changed.len());
    slots.resize_with(changed.len(), || None);
    let mut misses: Vec<(usize, FactsKey)> = Vec::new();
    for (index, (_, content, _)) in changed.iter().enumerate() {
        let key = FactsKey {
            content: *content,
            feature_fingerprint: FeatureSet::for_language_version(versions[index]).fingerprint(),
            semantic_version: FACTS_SEMANTIC_VERSION,
        };
        if let Some(facts) = state.cached_facts(key) {
            slots[index] = Some((facts, true));
        } else {
            misses.push((index, key));
        }
    }

    // Sources out of the pending batch (owned strings already in hand), then
    // the pure fan-out: no shared mutable state but the StringTable's
    // interning lock.
    let sources: Vec<&str> = changed.iter().map(|(_, _, text)| text.as_str()).collect();
    let computed: Vec<(usize, FactsKey, UnitFacts)> = misses
        .par_iter()
        .map(|(index, key)| {
            let facts = compute_unit(
                changed[*index].0,
                changed[*index].1,
                sources[*index],
                versions[*index],
                &state.strings,
            );
            (*index, *key, facts)
        })
        .collect();

    // Sequential URI resolution over the shared FileTable, then cache insert.
    let mut resolver = state.uri_resolver();
    let mut to_cache: Vec<(FactsKey, Arc<UnitFacts>)> = Vec::with_capacity(computed.len());
    for (index, key, mut facts) in computed {
        resolve_unit(&mut facts, changed[index].0, &mut resolver);
        let facts = Arc::new(facts);
        to_cache.push((key, Arc::clone(&facts)));
        slots[index] = Some((facts, false));
    }
    drop(resolver);
    for (key, facts) in to_cache {
        state.cache_facts(key, facts);
    }

    changed
        .iter()
        .zip(slots)
        .map(|((file, _, _), slot)| {
            let (facts, cache_hit) = slot.expect("every changed file ends with facts");
            (*file, facts, cache_hit)
        })
        .collect()
}

/// The per-file language version: owning package version, or the default
/// when the file belongs to no package (engine-api-spec §1.1).
fn language_version_for(state: &EngineState, file: FileId) -> LanguageVersion {
    state
        .files
        .path(file)
        .and_then(|path| state.packages.containing(path))
        .and_then(|identity| identity.language_version)
        .map(dartq_pub::LanguageVersion::into_inner)
        .unwrap_or(DEFAULT_LANGUAGE_VERSION)
}

/// The pure per-file pipeline: scan (with the `@dart` override rescan the
/// scanner's consumer contract mandates), directive diet walk, parse
/// diagnostics, element collection.
fn compute_unit(
    file: FileId,
    content: ContentId,
    text: &str,
    version: LanguageVersion,
    strings: &StringTable,
) -> UnitFacts {
    let mut feature_set = FeatureSet::for_language_version(version);
    let mut scanned = scan(text, scan_config_for(feature_set, version), file);
    if let Some(over) = scanned.language_version_override {
        if over != version {
            feature_set = FeatureSet::for_language_version(over);
            scanned = scan(text, scan_config_for(feature_set, over), file);
        }
    }

    let directives = extract_directives(text, &scanned.tokens, strings);
    let mut diagnostics = scanned.diagnostics;
    diagnostics.extend(parse_diagnostics(file, text, &scanned.tokens, feature_set));
    let elements = dartq_semantic::collect(
        text,
        &scanned.tokens,
        ParserFeatures::from_feature_set(feature_set),
        strings,
    );

    UnitFacts {
        content,
        directives,
        elements,
        diagnostics,
        line_index: scanned.line_index,
    }
}

/// The scanner's two lexical gates, from the feature set in force.
fn scan_config_for(feature_set: FeatureSet, version: LanguageVersion) -> ScanConfig {
    ScanConfig {
        enable_triple_shift: triple_shift_enabled(version),
        enable_augmentations: feature_set.is_enabled(dartq_language::Feature::Augmentations),
    }
}

/// Fills the resolution columns of a unit's directives and bakes the
/// URI-layer diagnostics (the only resolution diagnostics that are a pure
/// function of the file's own content — everything cross-file belongs to the
/// semantic seam).
fn resolve_unit(facts: &mut UnitFacts, file: FileId, resolver: &mut UriResolver<'_>) {
    // The engine extracts no `@docImport`s at M1, so every resolvable kind
    // maps to the resolution face's `DirectiveKind::ImportExport`; a
    // docImport extraction only gains a branch here. Authority-carrying
    // `dart:` URIs need no pre-validation either: they compose to an empty
    // library name, fail closed in `SdkSnapshot::uri_to_path`, and surface as
    // `uri_does_not_exist` — the oracle's own outcome for the same input
    // (`internalMapDartUri` parses the `dart:` prefix as the library name and
    // finds no such library).
    for directive in &mut facts.directives {
        if matches!(
            directive.kind,
            DirectiveKind::Import
                | DirectiveKind::Export
                | DirectiveKind::Part
                | DirectiveKind::PartOf
        ) {
            let UriResolution { target, code, .. } = resolver.resolve(
                file,
                directive.uri.as_deref(),
                dartq_resolution::DirectiveKind::ImportExport,
            );
            directive.target = target;
            directive.code = code;
        }
    }
    for directive in &facts.directives {
        if let Some(code) = directive.code {
            facts.diagnostics.push(uri_diagnostic(
                code,
                directive.uri.as_deref().unwrap_or_default(),
                directive.uri_span,
                file,
            ));
        }
    }
}

/// One URI-layer diagnostic. Severity: the spec-pinned levels where the
/// frozen table carries one, and `COMPILE_TIME_ERROR` for the remaining
/// URI-layer codes (they are all `CompileTimeErrorCode` rows in the
/// analyzer registry — an emit-layer decision on the unpinned rows).
pub(crate) fn uri_diagnostic(
    code: dartq_resolution::ResolutionCode,
    uri: &str,
    range: SourceRange,
    file: FileId,
) -> Diagnostic {
    let info = code.info();
    let severity = match code.pinned_severity() {
        Some("STATIC_WARNING") => Severity::Warning,
        _ => Severity::Error,
    };
    Diagnostic::new(
        severity,
        CodeInfo::new(
            info.name,
            info.machine,
            DiagnosticType::CompileTimeError,
            false,
        ),
        info.message.replace("{0}", uri),
        file,
        range,
        None,
    )
}

/// Parse-recovery diagnostics through the engine's listener (pass 1 of 2;
/// see `recompute_facts` for the two-pass note). The listener moves into the
/// parser (the trait is not object-safe over `&mut` here), so diagnostics
/// flow through a shared `Rc<RefCell>` — the same pattern
/// `dartq_semantic::collect` uses.
fn parse_diagnostics(
    file: FileId,
    text: &str,
    tokens: &TokenTable,
    feature_set: FeatureSet,
) -> Vec<Diagnostic> {
    let diags = Rc::new(RefCell::new(Vec::new()));
    Parser::new(
        text,
        tokens,
        ParserFeatures::from_feature_set(feature_set),
        DiagCollector {
            file,
            diags: Rc::clone(&diags),
        },
    )
    .parse_unit();
    Rc::try_unwrap(diags)
        .expect("parser released its listener")
        .into_inner()
}

/// Collects `handle_recoverable_error` events into engine diagnostics.
/// `handle_experiment_not_enabled` stays a no-op: its payload is a bare
/// feature flag without a code+text pair, and inventing wording here would
/// break oracle text parity — the message-capable path lands with the
/// diagnostic tickets.
struct DiagCollector {
    file: FileId,
    diags: Rc<RefCell<Vec<Diagnostic>>>,
}

impl ParserListener for DiagCollector {
    fn handle_recoverable_error(&mut self, message: Message, start: TokenSpan, _end: TokenSpan) {
        self.diags.borrow_mut().push(Diagnostic::new(
            Severity::Error,
            message.code,
            message.text,
            self.file,
            start.range,
            None,
        ));
    }
}

// ---------------------------------------------------------------------------
// Directive diet walk
//
// The parser consumes directives diet-style until its directive milestone,
// and emits no directive events. The engine therefore walks the directive
// region of the token table itself: after an optional script tag, metadata,
// and `;`s, `library`/`import`/`export`/`part` keyword tokens open directive
// shapes consumed to their `;`; any other token ends the region (the first
// declaration). Malformed shapes (a directive with no string) yield a
// directive with `uri: None` and no diagnostic — under-reporting, matching
// the "never false-positive" bias until the parser milestone owns these.
// ---------------------------------------------------------------------------

/// Walks the directive region of a token table.
fn extract_directives(
    text: &str,
    tokens: &TokenTable,
    strings: &StringTable,
) -> Vec<DirectiveFacts> {
    let mut directives = Vec::new();
    let mut i = 0;
    loop {
        let Some(token) = tokens.get(i) else { break };
        match token.kind {
            TokenKind::Eof => break,
            TokenKind::Semicolon | TokenKind::ScriptTag => i += 1,
            TokenKind::At => i = skip_annotation(tokens, i + 1),
            TokenKind::Keyword(Keyword::Library) => {
                i = walk_library(text, tokens, i, strings, &mut directives);
            }
            TokenKind::Keyword(Keyword::Import) => {
                i = walk_import_export(
                    text,
                    tokens,
                    i,
                    strings,
                    &mut directives,
                    DirectiveKind::Import,
                );
            }
            TokenKind::Keyword(Keyword::Export) => {
                i = walk_import_export(
                    text,
                    tokens,
                    i,
                    strings,
                    &mut directives,
                    DirectiveKind::Export,
                );
            }
            TokenKind::Keyword(Keyword::Part) => {
                i = walk_part(text, tokens, i, strings, &mut directives);
            }
            _ => break,
        }
    }
    directives
}

/// The token kind at `i`, if any.
fn kind_at(tokens: &TokenTable, i: usize) -> Option<TokenKind> {
    tokens.get(i).map(|token| token.kind)
}

/// Whether `kind` can spell part of an annotation name: identifiers and
/// built-in identifiers (`dynamic`) only — reserved/pseudo keywords would
/// swallow the next declaration.
fn is_name_token(kind: TokenKind) -> bool {
    match kind {
        TokenKind::Identifier => true,
        TokenKind::Keyword(keyword) => keyword.style() == KeywordStyle::BuiltIn,
        _ => false,
    }
}

/// Skips `@qualifiedName[(args)]` starting after the `@`.
fn skip_annotation(tokens: &TokenTable, mut i: usize) -> usize {
    while kind_at(tokens, i).is_some_and(is_name_token)
        || kind_at(tokens, i) == Some(TokenKind::Period)
    {
        i += 1;
    }
    if kind_at(tokens, i) == Some(TokenKind::OpenParen) {
        i = tokens.matching_end(i).map_or(i, |end| end + 1);
    }
    i
}

/// A dotted name (`a.b.c`) of identifier-ish tokens, consumed to the `;`.
/// Returns the joined spelling and the index just past the `;`.
fn walk_dotted_name(text: &str, tokens: &TokenTable, mut i: usize) -> (Option<Box<str>>, usize) {
    let mut name = String::new();
    loop {
        match kind_at(tokens, i) {
            Some(TokenKind::Semicolon) | None => return (Some(name.into_boxed_str()), i + 1),
            Some(kind) if is_name_token(kind) => {
                if !name.is_empty() {
                    name.push('.');
                }
                name.push_str(tokens.lexeme(text, i));
                i += 1;
            }
            Some(TokenKind::Period) => i += 1,
            _ => return (Some(name.into_boxed_str()), i + 1),
        }
    }
}

/// `library [a.b.c];`
fn walk_library(
    text: &str,
    tokens: &TokenTable,
    start: usize,
    _strings: &StringTable,
    out: &mut Vec<DirectiveFacts>,
) -> usize {
    let (name, end) = walk_dotted_name(text, tokens, start + 1);
    out.push(DirectiveFacts {
        kind: DirectiveKind::Library,
        uri: None,
        uri_span: SourceRange {
            offset: 0,
            length: 0,
        },
        target: ResolvedTarget::Missing,
        code: None,
        prefix: None,
        deferred: false,
        combinators: Combinators::unfiltered(),
        name,
    });
    end
}

/// Reads the URI literal starting at `i`, handling the interpolation split
/// (a `String` segment followed by an interpolation token carries no
/// resolvable string). Returns the uri text, its UTF-16 span, and the index
/// just past the literal region.
fn read_uri(text: &str, tokens: &TokenTable, i: usize) -> (Option<Box<str>>, SourceRange, usize) {
    let interpolated = |kind: Option<TokenKind>| {
        matches!(
            kind,
            Some(
                TokenKind::StringInterpolationExpression | TokenKind::StringInterpolationIdentifier
            )
        )
    };
    if kind_at(tokens, i) != Some(TokenKind::String) {
        return (
            None,
            SourceRange {
                offset: 0,
                length: 0,
            },
            i,
        );
    }
    if interpolated(kind_at(tokens, i + 1)) {
        // Skip the whole segment chain: `String` pieces and `${ … }` groups
        // (group-aware, so a `;` inside the interpolation cannot mislead).
        let mut j = i;
        loop {
            match kind_at(tokens, j) {
                Some(TokenKind::String | TokenKind::StringInterpolationIdentifier) => j += 1,
                Some(TokenKind::StringInterpolationExpression) => {
                    j = tokens.matching_end(j).map_or(j + 1, |end| end + 1);
                }
                _ => break,
            }
        }
        return (
            None,
            SourceRange {
                offset: 0,
                length: 0,
            },
            j,
        );
    }
    let token = tokens.get(i).expect("kind_at checked");
    let lexeme = tokens.lexeme(text, i);
    let span = SourceRange {
        offset: token.utf16_offset,
        length: utf16_len_of(lexeme),
    };
    (string_literal_value(lexeme).map(Into::into), span, i + 1)
}

/// `import`/`export`: uri, `[deferred] [as p]` (import shapes only in
/// practice, tolerated on both), combinator list, `;`.
fn walk_import_export(
    text: &str,
    tokens: &TokenTable,
    start: usize,
    strings: &StringTable,
    out: &mut Vec<DirectiveFacts>,
    kind: DirectiveKind,
) -> usize {
    let (uri, uri_span, mut i) = read_uri(text, tokens, start + 1);
    let mut prefix = None;
    let mut deferred = false;
    let mut clauses: Vec<CombinatorClause> = Vec::new();
    loop {
        match kind_at(tokens, i) {
            None | Some(TokenKind::Eof) => break,
            Some(TokenKind::Semicolon) => {
                i += 1;
                break;
            }
            Some(TokenKind::Keyword(Keyword::Deferred)) => {
                deferred = true;
                i += 1;
            }
            Some(TokenKind::Keyword(Keyword::As)) => {
                if let Some(next) = kind_at(tokens, i + 1) {
                    if is_name_token(next) || matches!(next, TokenKind::Keyword(_)) {
                        prefix = Some(strings.intern(tokens.lexeme(text, i + 1)));
                        i += 2;
                        continue;
                    }
                }
                i += 1;
            }
            Some(TokenKind::Keyword(Keyword::Show | Keyword::Hide)) => {
                let hide = kind_at(tokens, i) == Some(TokenKind::Keyword(Keyword::Hide));
                i += 1;
                let mut names = Vec::new();
                while let Some(next) = kind_at(tokens, i) {
                    match next {
                        TokenKind::Identifier => {
                            names.push(strings.intern(tokens.lexeme(text, i)));
                            i += 1;
                        }
                        TokenKind::Comma => i += 1,
                        _ => break,
                    }
                }
                clauses.push(if hide {
                    CombinatorClause::Hide(names.into())
                } else {
                    CombinatorClause::Show(names.into())
                });
            }
            _ => i += 1,
        }
    }
    out.push(DirectiveFacts {
        kind,
        uri,
        uri_span,
        target: ResolvedTarget::Missing,
        code: None,
        prefix,
        deferred,
        combinators: Combinators::new(clauses, strings),
        name: None,
    });
    i
}

/// `part 'u';` and `part of 'u' | a.b.c;`
fn walk_part(
    text: &str,
    tokens: &TokenTable,
    start: usize,
    _strings: &StringTable,
    out: &mut Vec<DirectiveFacts>,
) -> usize {
    if kind_at(tokens, start + 1) == Some(TokenKind::Keyword(Keyword::Of)) {
        if kind_at(tokens, start + 2) == Some(TokenKind::String) {
            let (uri, uri_span, mut i) = read_uri(text, tokens, start + 2);
            if kind_at(tokens, i) == Some(TokenKind::Semicolon) {
                i += 1;
            }
            out.push(DirectiveFacts {
                kind: DirectiveKind::PartOf,
                uri,
                uri_span,
                target: ResolvedTarget::Missing,
                code: None,
                prefix: None,
                deferred: false,
                combinators: Combinators::unfiltered(),
                name: None,
            });
            return i;
        }
        let (name, end) = walk_dotted_name(text, tokens, start + 2);
        out.push(DirectiveFacts {
            kind: DirectiveKind::PartOf,
            uri: None,
            uri_span: SourceRange {
                offset: 0,
                length: 0,
            },
            target: ResolvedTarget::Missing,
            code: None,
            prefix: None,
            deferred: false,
            combinators: Combinators::unfiltered(),
            name,
        });
        return end;
    }
    let (uri, uri_span, mut i) = read_uri(text, tokens, start + 1);
    if kind_at(tokens, i) == Some(TokenKind::Semicolon) {
        i += 1;
    }
    out.push(DirectiveFacts {
        kind: DirectiveKind::Part,
        uri,
        uri_span,
        target: ResolvedTarget::Missing,
        code: None,
        prefix: None,
        deferred: false,
        combinators: Combinators::unfiltered(),
        name: None,
    });
    i
}

/// Strips quotes (raw and cooked, single/double, single/triple) from a
/// complete string-literal lexeme and processes escapes of cooked bodies.
/// Returns `None` for anything that is not one complete literal.
fn string_literal_value(literal: &str) -> Option<String> {
    let is_raw = literal.starts_with('r');
    let body = if is_raw { &literal[1..] } else { literal };
    let body = if let Some(rest) = body.strip_prefix("\"\"\"") {
        rest.strip_suffix("\"\"\"")?
    } else if let Some(rest) = body.strip_prefix("'''") {
        rest.strip_suffix("'''")?
    } else if let Some(rest) = body.strip_prefix('"') {
        rest.strip_suffix('"')?
    } else {
        body.strip_prefix('\'')?.strip_suffix('\'')?
    };
    if is_raw {
        return Some(body.to_owned());
    }
    unescape(body)
}

/// Dart cooked-string escape processing (`\n`-style short escapes, `\xHH`,
/// `\uHHHH`, `\u{H…}`, `\<char>` passthrough, and backslash-newline elision
/// in multiline bodies).
fn unescape(body: &str) -> Option<String> {
    let chars: Vec<char> = body.chars().collect();
    let mut out = String::with_capacity(body.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        i += 1;
        if c != '\\' {
            out.push(c);
            continue;
        }
        match *chars.get(i)? {
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            'f' => out.push('\u{0C}'),
            'b' => out.push('\u{08}'),
            't' => out.push('\t'),
            'v' => out.push('\u{0B}'),
            'x' => {
                let hi = *chars.get(i + 1)?;
                let lo = *chars.get(i + 2)?;
                let digits = format!("{hi}{lo}");
                i += 2;
                out.push(char::from_u32(u32::from_str_radix(&digits, 16).ok()?)?);
            }
            'u' => {
                i += 1;
                let digits = if chars.get(i) == Some(&'{') {
                    let start = i + 1;
                    let mut end = start;
                    while end < chars.len() && chars.get(end) != Some(&'}') {
                        end += 1;
                    }
                    if end == chars.len() {
                        return None;
                    }
                    i = end;
                    chars[start..end].iter().collect::<String>()
                } else {
                    if i + 4 > chars.len() {
                        return None;
                    }
                    let digits = chars[i..i + 4].iter().collect::<String>();
                    i += 3;
                    digits
                };
                out.push(char::from_u32(u32::from_str_radix(&digits, 16).ok()?)?);
            }
            '\n' => {}
            '\r' => {
                if chars.get(i + 1) == Some(&'\n') {
                    i += 1;
                }
            }
            other => out.push(other),
        }
        i += 1;
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// Library reformation + cascade closure
// ---------------------------------------------------------------------------

/// Rebuilds the library set from residency, recomputes reverse import edges,
/// and derives the post-closure dirty set: seeds are libraries with no
/// cached result or a changed identity — member contents, dependency set,
/// and the unclaimed part targets (§3.2 steps 3–5). Old and new identities
/// are exact lists, so every membership, edge, or part-target residency
/// change shows up as a difference on the library that owns it; no separate
/// directive diff is needed.
fn reform_and_close(state: &mut EngineState) -> Vec<LibraryId> {
    reform_libraries(state);
    close(state)
}

/// Rebuilds `libraries`, `library_of_file`, and `importers` from residency.
///
/// Formation: defining files (no `part of`) in `FileId` order each own a
/// library keyed on their canonical URI; their `part` targets that are
/// themselves part files join in directive order (first claim wins on
/// conflicts — `duplicate_part` is a diagnostic ticket's business). Part
/// files claimed by nobody become singleton libraries. Every resident file
/// belongs to exactly one library (engine-api-spec §1.2).
fn reform_libraries(state: &mut EngineState) {
    // Canonical URIs are a pure function of the interned path; memoize per
    // file (a file's path never changes once interned).
    let mut resident: Vec<FileId> = state.residency.keys().copied().collect();
    resident.sort_unstable_by_key(|file| file.index());
    for file in &resident {
        if !state.canonical_uris.contains_key(file) {
            let path = state
                .files
                .path(*file)
                .map_or_else(std::path::PathBuf::new, std::path::Path::to_path_buf);
            let uri = state.uri_resolver().canonical_uri(&path);
            state.canonical_uris.insert(*file, uri);
        }
    }

    // Pass A: defining libraries (FileId order fixes claim precedence).
    let mut defining: Vec<(FileId, LibraryId)> = Vec::new();
    let mut is_part: FxHashSet<FileId> = FxHashSet::default();
    for file in &resident {
        if state.residency[file].is_part() {
            is_part.insert(*file);
        } else {
            let id = state.libraries.intern(state.canonical_uris[file].clone());
            defining.push((*file, id));
        }
    }

    // Pass B: attach parts (directive order, first claim wins).
    let mut claims: FxHashMap<FileId, LibraryId> = FxHashMap::default();
    let mut parts_of: FxHashMap<LibraryId, Vec<FileId>> = FxHashMap::default();
    for (defining_file, lib) in &defining {
        for directive in &state.residency[defining_file].directives {
            if directive.kind != DirectiveKind::Part {
                continue;
            }
            if let ResolvedTarget::Resolved(target) = directive.target
                && is_part.contains(&target)
                && state.residency.contains_key(&target)
                && !claims.contains_key(&target)
            {
                claims.insert(target, *lib);
                parts_of.entry(*lib).or_default().push(target);
            }
        }
    }

    // Pass C: orphaned part files become singleton libraries.
    for file in &resident {
        if is_part.contains(file) && !claims.contains_key(file) {
            let id = state.libraries.intern(state.canonical_uris[file].clone());
            defining.push((*file, id));
        }
    }

    // File membership (every resident file, exactly one library).
    let mut library_of_file: FxHashMap<FileId, LibraryId> = FxHashMap::default();
    for (file, lib) in &defining {
        library_of_file.insert(*file, *lib);
    }
    for (file, lib) in &claims {
        library_of_file.insert(*file, *lib);
    }

    // Pass D: content identity (defining + parts, directive order) and
    // dependency set (resolved import/export targets' owning libraries).
    let mut infos: Vec<(LibraryId, LibraryInfo)> = Vec::with_capacity(defining.len());
    for (file, lib) in &defining {
        let parts = parts_of
            .get(lib)
            .into_iter()
            .flatten()
            .copied()
            .collect::<Vec<_>>();
        let mut members: Vec<FileId> = vec![*file];
        members.extend_from_slice(&parts);
        let content: Box<[ContentId]> = members
            .iter()
            .map(|member| state.residency[member].content)
            .collect();
        let mut deps: Vec<LibraryId> = Vec::new();
        for member in &members {
            for directive in &state.residency[member].directives {
                if matches!(
                    directive.kind,
                    DirectiveKind::Import | DirectiveKind::Export
                ) && let ResolvedTarget::Resolved(target) = directive.target
                    && let Some(dep) = library_of_file.get(&target)
                    && *dep != *lib
                {
                    deps.push(*dep);
                }
            }
        }
        deps.sort_unstable();
        deps.dedup();
        // Part directives resolving to a resident file outside membership:
        // neither content (membership) nor deps (import/export only) sees
        // them, so their residency flip is carried by the identity
        // explicitly. Without it, removing such a target leaves the library
        // clean while its cached result predates the URI going dangling —
        // incremental diverges from a full recompute, which bakes
        // `uri_does_not_exist` against the absent target.
        let mut part_targets: Vec<FileId> = Vec::new();
        for member in &members {
            for directive in &state.residency[member].directives {
                if directive.kind == DirectiveKind::Part
                    && let ResolvedTarget::Resolved(target) = directive.target
                    && state.residency.contains_key(&target)
                    && !members.contains(&target)
                {
                    part_targets.push(target);
                }
            }
        }
        part_targets.sort_unstable_by_key(|file| file.index());
        part_targets.dedup();
        infos.push((
            *lib,
            LibraryInfo {
                uri: state.canonical_uris[file].clone(),
                defining: *file,
                parts: parts.into(),
                content,
                deps,
                part_targets: part_targets.into(),
            },
        ));
    }

    // Reverse edges from the new directive set.
    let mut importers: FxHashMap<FileId, FxHashSet<LibraryId>> = FxHashMap::default();
    for (file, lib) in &library_of_file {
        for directive in &state.residency[file].directives {
            if matches!(
                directive.kind,
                DirectiveKind::Import | DirectiveKind::Export
            ) && let ResolvedTarget::Resolved(target) = directive.target
                && state.residency.contains_key(&target)
            {
                importers.entry(target).or_default().insert(*lib);
            }
        }
    }

    // Write back: retire gone libraries (their results go with them),
    // replace infos, and swap the derived maps.
    let live: FxHashSet<LibraryId> = defining.iter().map(|(_, lib)| *lib).collect();
    let retired = state.libraries.retire_except(&live);
    for id in retired {
        state.results.remove(&id);
    }
    for (id, info) in infos {
        state.libraries.set_info(id, info);
    }
    state.library_of_file = library_of_file;
    state.importers = importers;
}

/// The post-closure dirty set: libraries with no cached result or changed
/// identity, closed along the reverse edges (§3.2 step 5).
fn close(state: &mut EngineState) -> Vec<LibraryId> {
    let mut dirty: FxHashSet<LibraryId> = FxHashSet::default();
    let mut work: Vec<LibraryId> = Vec::new();
    for lib in state.libraries.live() {
        let info = state.libraries.info(lib).expect("live library has info");
        let stale = match state.results.get(&lib) {
            None => true,
            Some(result) => {
                result.content != info.content
                    || result.deps != info.deps
                    || result.part_targets != info.part_targets
            }
        };
        if stale {
            dirty.insert(lib);
            work.push(lib);
        }
    }
    while let Some(lib) = work.pop() {
        let info = state.libraries.info(lib).expect("live library has info");
        for file in std::iter::once(info.defining).chain(info.parts.iter().copied()) {
            if let Some(importer_set) = state.importers.get(&file) {
                for importer in importer_set {
                    if dirty.insert(*importer) {
                        work.push(*importer);
                    }
                }
            }
        }
    }
    let mut invalidated: Vec<LibraryId> = dirty.into_iter().collect();
    invalidated.sort_unstable();
    invalidated
}
