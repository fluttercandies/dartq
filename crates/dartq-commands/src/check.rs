//! The check face's core: the M0 pipeline over a
//! discovered file set — bytes → scan → `@dart` override rescan → parse —
//! plus the URI-layer existence face for relative directive targets:
//! `uri_does_not_exist` / `uri_has_not_been_generated` where a sibling the
//! directive names is absent, the one resolution diagnostic decidable from
//! the file set alone. Producing the canonical-order diagnostic set the
//! oracle-exact writers render, it stops there by design: no
//! pub/package/SDK/engine involvement (scheme-carrying URIs — `dart:`,
//! `package:` — need the package/SDK universe and stay the engine view's
//! business). The CLI/RPC equivalence is pinned by both faces sharing
//! this implementation.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::layout::{path_key, severity_rank};
use dartq_diagnostics::{
    AnalysisOutput, CodeInfo, CodeRegistry, Diagnostic, FileId, FileLayoutProvider, Severity,
    SourceRange, write_machine,
};
use dartq_language::{
    DEFAULT_LANGUAGE_VERSION, Feature, FeatureSet, LanguageVersion,
    MIN_LANGUAGE_VERSION_CONSTRAINT, triple_shift_enabled,
};
use dartq_parser::{
    ExperimentalFlag, Message, Parser, ParserFeatures, ParserListener, TokenSpan,
    experiment_not_enabled_message,
};
use dartq_resolution::{ResolutionCode, parse_directive_uri};
use dartq_scanner::{Keyword, KeywordStyle, ScanConfig, ScanResult, TokenKind, TokenTable, scan};
use dartq_source::{
    FileTable, LineIndex, MAX_SOURCE_BYTES, OpenError, normalize_lexical, utf16_len_of,
};

/// One check run's result: the per-file layout state behind the writers and
/// the oracle-order diagnostic set (severity, then path bytes, then source
/// position, then message — the upstream `DiagnosticWithPath.compareTo`
/// granularity).
pub struct CheckRun {
    files: FileSet,
    diagnostics: Vec<Diagnostic>,
}

impl CheckRun {
    /// The writers' input: diagnostics plus the file layout they resolve
    /// positions against.
    #[must_use]
    pub fn output(&self) -> AnalysisOutput<'_> {
        AnalysisOutput {
            diagnostics: &self.diagnostics,
            files: &self.files,
        }
    }

    /// The canonical-order diagnostic set.
    #[must_use]
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    /// The number of analyzed files.
    #[must_use]
    pub fn file_count(&self) -> usize {
        self.files.sources.len()
    }

    /// Total lines of the analyzed sources: newline bytes counted in one
    /// pass (the engine discovery convention, no line splitting).
    #[must_use]
    pub fn loc(&self) -> u64 {
        self.files
            .sources
            .iter()
            .map(|source| source.bytes().filter(|&b| b == b'\n').count() as u64)
            .sum()
    }
}

// ---------------------------------------------------------------------------
// The verbosity-tiered machine face
//
// The machine rows' output volume (`-v`/`--verbosity` on the CLI's `check`
// and `analyze` faces; the token-cost sweep's tier axis). Rendering-only by
// contract: the diagnostic set itself is complete and identical at every
// tier — the same posture as the memory-budget rows (diagnostics stay
// complete while the output volume moves).
// ---------------------------------------------------------------------------

/// The machine rows' output volume (the token-cost sweep's tier axis).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticVerbosity {
    /// Code + span rows: the message field drops, the identifying
    /// `SEVERITY|TYPE|CODE|FILE|LINE|COL|LENGTH` prefix stays.
    Concise,
    /// The oracle machine row verbatim (the default — the byte-parity
    /// differential face).
    Default,
    /// The oracle machine row plus a ninth field: the correctionMessage
    /// (`-` when the template carries none).
    Verbose,
}

/// Writes the machine rows at `verbosity` — the one renderer the `check`
/// and `analyze` faces share. The default tier delegates to the oracle-exact
/// [`write_machine`], so byte parity with the pre-tier output is structural,
/// not re-rendered.
///
/// # Errors
/// Propagates any I/O error surfaced by `out`.
pub fn write_machine_verbosity(
    out: &mut impl io::Write,
    output: &AnalysisOutput<'_>,
    verbosity: DiagnosticVerbosity,
) -> io::Result<()> {
    use std::fmt::Write as _;

    if verbosity == DiagnosticVerbosity::Default {
        return write_machine(out, output);
    }

    let mut field = String::new();
    for d in output.diagnostics {
        let (line, column) = output.files.line_col(d.file, d.range.offset);
        // The prefix through LENGTH is the tiers' common spine, assembled and
        // flushed exactly like the oracle writer (one `write_all` per row).
        field.clear();
        field.push_str(d.severity.as_machine_str());
        field.push('|');
        field.push_str(d.code.dtype.as_str());
        field.push('|');
        field.push_str(d.code.machine);
        field.push('|');
        escape_machine_field(
            output.files.file_path(d.file).to_string_lossy().as_ref(),
            &mut field,
        );
        // Infallible: `String`'s `fmt::Write` never fails.
        let _ = write!(field, "|{line}|{column}|{}", d.range.length);
        if verbosity != DiagnosticVerbosity::Concise {
            // `Default` returned above, so this branch is `Verbose`: the
            // message rides as the eighth field — the oracle writer's
            // rendering verbatim — and the correctionMessage follows as
            // the ninth, `-` when absent (the query face's
            // absent-container posture), both escaped like every other
            // free-text field.
            field.push('|');
            escape_machine_field(&d.message, &mut field);
            field.push('|');
            match d.correction.as_deref() {
                Some(correction) => escape_machine_field(correction, &mut field),
                None => field.push('-'),
            }
        }
        field.push('\n');
        out.write_all(field.as_bytes())?;
    }
    Ok(())
}

/// Machine-format field escaping — `dartq-diagnostics`' private
/// `writer.rs::escape_into` mirrored byte-for-byte: `\` → `\\`, `|` → `\|`,
/// newline → literal `\n`, CR → literal `\r`, everything else verbatim.
fn escape_machine_field(text: &str, out: &mut String) {
    let bytes = text.as_bytes();
    let mut copied = 0;
    for (i, &b) in bytes.iter().enumerate() {
        let escaped = match b {
            b'\\' => "\\\\",
            b'|' => "\\|",
            b'\n' => "\\n",
            b'\r' => "\\r",
            _ => continue,
        };
        out.push_str(&text[copied..i]);
        out.push_str(escaped);
        copied = i + 1;
    }
    out.push_str(&text[copied..]);
}

/// The canonical file's parent directory — relative directive targets
/// compose against it. `Path::parent` is `None` only for a root, and a root
/// is never a file [`read_source`] accepts, so the `None` arm is a
/// defensive degrade: a structured error, never a panic.
fn parent_dir(canonical: &Path) -> io::Result<&Path> {
    canonical.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("path has no parent: {}", canonical.display()),
        )
    })
}

/// Runs the M0 pipeline over a discovered file set (see
/// [`crate::discover::discover`]) — the bare-list face: no discovery
/// targets are known, so the analysis root falls back to
/// [`crate::pkg_label::common_root`] over the canonical set (the deepest
/// covering directory). Callers that still hold the targets the set was
/// discovered from route through [`analyze_targets`] instead, whose root
/// is the target directory itself.
///
/// A read failure aborts the run — a partial diagnostic set must never
/// masquerade as a valid result; an undecodable file is not a partial
/// result but the oracle's own empty-content face (see [`read_source`]).
///
/// Duplicate spellings are analyzed once: the discovery set's dedup is
/// component-wise, so a file whose spellings differ as component vectors (a
/// `..`-carrying explicit arg next to its walked entry, or a symlink whose
/// target is also walked) arrives twice; the canonical-dedup face below
/// keeps one analyzed id per file, the oracle walk's canonical-path dedup
/// (context_root.dart:119-129).
///
/// # Errors
///
/// Source read or UTF-8 decode failure, or a canonical path without a
/// parent directory (defensive — see [`parent_dir`]).
pub fn analyze_files(files: &BTreeSet<PathBuf>) -> Result<CheckRun, io::Error> {
    analyze_targets(&[], files)
}

/// [`analyze_files`] over a discovery that started at `targets` (the check
/// face's path arguments): the analysis root is the deepest directory
/// covering the targets — the directory the run was pointed at, the
/// enclosing package root in the common CLI shape — so rows spell under
/// the target (`lib/main.dart`, not `main.dart` under a `lib/` root the
/// file set alone would derive). An empty target list is the bare-list
/// face (see [`analyze_files`]).
///
/// # Errors
///
/// Source read or UTF-8 decode failure, or a canonical path without a
/// parent directory (defensive — see [`parent_dir`]).
pub fn analyze_targets(
    targets: &[PathBuf],
    files: &BTreeSet<PathBuf>,
) -> Result<CheckRun, io::Error> {
    let mut files_state = FileSet::default();
    let mut diags = Vec::new();
    let mut canonical_paths = Vec::new();
    for path in files {
        let (canonical, source) = read_source(path)?;
        if files_state.file_id(&canonical).is_some() {
            continue;
        }
        canonical_paths.push(canonical.clone());
        let file = files_state.push(&source, &canonical);
        // Relative directive targets compose against the canonical file's
        // directory (canonical paths are absolute, so the parent exists).
        let dir = parent_dir(&canonical)?;
        diags.extend(analyze_file(&source, dir, file));
    }
    // The oracle display order (DiagnosticWithPath.compareTo, upstream Dart
    // SDK source, pkg/dartdev/lib/src/commands/analyze.dart:681-705):
    // severity first, then the printed path's bytes, then position (the
    // UTF-16 offset stands in for the monotone line/character pair), then
    // message — no code tiebreak exists upstream, and `emitMachineFormat`
    // re-sorts the merged list through the same comparator (:338).
    // analyze.rs's `oracle_cmp` is the sibling implementation over the
    // engine layout. The path bytes lead within one severity — the printed
    // path, not the intern slot: the intern order follows the discovery
    // set's raw spellings, so under a symlinked duplicate the deduped
    // file's slot can precede a lexically smaller real path while its
    // canonical name sorts after (#558).
    diags.sort_by(|a, b| {
        severity_rank(a.severity)
            .cmp(&severity_rank(b.severity))
            .then_with(|| {
                path_key(files_state.file_path(a.file)).cmp(path_key(files_state.file_path(b.file)))
            })
            .then_with(|| a.range.offset.cmp(&b.range.offset))
            .then_with(|| a.message.cmp(&b.message))
    });
    // The check face's analysis root: the target directory when the call
    // site knows one, the deepest covering directory of the canonical set
    // otherwise (the lon/text locator's projection base either way).
    files_state.root = crate::pkg_label::target_root(targets)
        .or_else(|| crate::pkg_label::common_root(&canonical_paths));
    Ok(CheckRun {
        files: files_state,
        diagnostics: diags,
    })
}

/// Canonical absolute path (the oracle prints symlink-resolved paths) plus
/// the source text. A read failure is an error, never a silently skipped
/// file.
///
/// A UTF-8 decode failure is not an error: the oracle read layer
/// (analyzer `file_content_cache.dart:44-62`) decodes strictly inside a
/// catch-all and lands the file as empty content with `exists = false` —
/// it contributes no diagnostics and the run continues over its siblings.
/// Lossy decode would scan U+FFFD debris the oracle never sees; aborting
/// the run would fault a whole corpus on one fixture.
///
/// The dedup key is the canonical path **string** — the oracle's own key
/// (context_root.dart:119-129 canonicalize), which the differential harness
/// mirrors on both sides. A samefile-style `(st_dev, st_ino)` identity
/// would diverge from the oracle on the case-insensitive volumes (macOS
/// APFS, Windows NTFS) where case-mismatched spellings of one file
/// canonicalize to two strings and legitimately double-emit, oracle alike —
/// the documented residual, kept for parity (#558).
///
/// Exactly one leading UTF-8 BOM is dropped (oracle read-layer parity: the
/// analyzer read path strips it before scanning; spans are relative to the
/// stripped text). A second BOM is content.
///
/// A file over [`MAX_SOURCE_BYTES`] is rejected on the span-capacity gate —
/// the bypass face of #1543: this read reaches [`LineIndex::new`] directly,
/// outside the engine's intake gates, so the same bound refuses it here.
/// The stat rejects before the bytes are read (nothing materializes to be
/// rejected); the byte length is the authoritative bound, covering a file
/// swapped between the two checks.
///
/// # Errors
///
/// Canonicalization or read failure, including the span-capacity
/// rejection.
pub fn read_source(path: &Path) -> io::Result<(PathBuf, String)> {
    let canonical = path.canonicalize()?;
    if let Some(error) = stat_capacity_error(&canonical) {
        return Err(error);
    }
    let bytes = fs::read(&canonical)?;
    let len = bytes.len();
    if len > MAX_SOURCE_BYTES {
        return Err(capacity_error(&canonical, len as u64));
    }
    let mut source = String::from_utf8(bytes).unwrap_or_default();
    if source.starts_with('\u{FEFF}') {
        source.drain(..'\u{FEFF}'.len_utf8());
    }
    Ok((canonical, source))
}

/// The span-capacity rejection for the command faces' own file reads —
/// the #1543 bypass face: [`read_source`] and the fix face's reads build
/// their [`LineIndex`] directly, outside the engine's intake gates, so the
/// same [`MAX_SOURCE_BYTES`] bound rejects here. The message carries the
/// path and the file's size via [`OpenError::TooLarge`]'s canonical text;
/// each face renders it through its own error channel.
pub(crate) fn capacity_error(path: &Path, len: u64) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("{}: {}", path.display(), OpenError::TooLarge { len }),
    )
}

/// The pre-read stat rejection — [`dartq_source::open_mapped`]'s gate
/// mirrored at the command faces' read entries: a file already oversized
/// on disk is refused before its bytes are read, so no face materializes
/// gigabytes only to reject them. `None` — including when the stat itself
/// fails — falls through to the read, whose error is the truth.
pub(crate) fn stat_capacity_error(path: &Path) -> Option<io::Error> {
    let len = fs::metadata(path).ok()?.len();
    (len > MAX_SOURCE_BYTES as u64).then(|| capacity_error(path, len))
}

/// Per-file state behind [`FileLayoutProvider`]: canonical paths live in the
/// [`FileTable`], positions resolve through the production [`LineIndex`] path
/// (the same conversion the diagnostics goldens pin).
#[derive(Default)]
pub struct FileSet {
    table: FileTable,
    sources: Vec<String>,
    indexes: Vec<LineIndex>,
    /// The run's analysis root (the lon/text locator's base), set by
    /// [`analyze_targets`] — the covering directory of the discovery
    /// targets, or of the canonical set when no targets are known; `None`
    /// for a hand-built set (the unit fixtures) or a degenerate root.
    root: Option<PathBuf>,
}

impl FileSet {
    /// Interns `path` (canonicalized by the caller) and registers its layout.
    pub fn push(&mut self, source: &str, path: &Path) -> FileId {
        let file = self.table.intern(path);
        debug_assert_eq!(file.index() as usize, self.sources.len());
        self.indexes.push(LineIndex::new(source));
        self.sources.push(source.to_owned());
        file
    }

    /// The id previously interned for `path` (canonicalized by the caller),
    /// if any — the canonical-dedup face of [`Self::push`].
    #[must_use]
    pub fn file_id(&self, path: &Path) -> Option<FileId> {
        self.table.try_file_id(path)
    }
}

impl FileLayoutProvider for FileSet {
    fn file_path(&self, file: FileId) -> &Path {
        self.table.path(file).expect("interned FileId")
    }

    fn analysis_root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    fn line_col(&self, file: FileId, utf16_offset: u32) -> (u32, u32) {
        let index = file.index() as usize;
        // Diagnostic offsets are UTF-16; the fused conversion reports the
        // official `LineInfo.getLocation` (line, UTF-16 column) directly.
        self.indexes[index].line_col_utf16(&self.sources[index], utf16_offset)
    }
}

/// Runs the M0 pipeline over one file: scan (+ override rescan), parse
/// through a diagnostics-collecting listener, then the relative-URI
/// existence face over the harvested directive targets.
fn analyze_file(source: &str, dir: &Path, file: FileId) -> Vec<Diagnostic> {
    let (version, scanned, mut diags) = scan_file(source, file);
    let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(version));
    // A shared sink collects events out of the parse. When #18 starts
    // emitting `handle_recoverable_error`, these diagnostics flow without
    // CLI changes.
    let sink = Rc::new(RefCell::new(Vec::new()));
    let mut listener = DiagnosticListener {
        file,
        sink: Rc::clone(&sink),
    };
    Parser::new(source, &scanned.tokens, features, &mut listener).parse_unit();
    diags.extend(std::mem::take(&mut *sink.borrow_mut()));
    diags.extend(directive_diagnostics(dir, file, &scanned.tokens, source));
    diags.extend(scanned.diagnostics);
    diags
}

// ---------------------------------------------------------------------------
// The relative-URI existence face
//
// The engine resolves directive URIs through `l1::resolve_unit` +
// `UriResolver` over the full package/SDK universe; the disk face has
// neither, so it judges exactly the decidable class — a scheme-less target
// composes against the file's own directory and must exist on disk
// (`uri_does_not_exist`, or `uri_has_not_been_generated` for the generated
// suffixes). The directive harvest restates the engine's diet walk
// (`l1::extract_directives`) minus the resolution bookkeeping; like it, the
// face never false-positives — shapes it cannot judge (interpolated,
// escape-carrying, percent-encoded, scheme-carrying URIs) stay silent.
// ---------------------------------------------------------------------------

/// One resolvable directive URI: the cooked value (adjacent literals
/// concatenated) and its UTF-16 span — the whole literal region, quotes
/// included (the engine's `read_uri` face).
struct DirectiveUri {
    value: String,
    span: SourceRange,
}

/// The URI diagnostics of one file: every harvested directive target that
/// is missing on disk.
fn directive_diagnostics(
    dir: &Path,
    file: FileId,
    tokens: &TokenTable,
    text: &str,
) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    for uri in harvest_directive_uris(text, tokens) {
        if let Some(diagnostic) = missing_relative(&uri, dir, file) {
            diags.push(diagnostic);
        }
    }
    diags
}

/// One directive URI's existence verdict: `Some` when its relative target
/// is missing on disk. Scheme-carrying URIs (`dart:`, `package:`, `file:`,
/// `data:`, …), protocol-relative authorities, and percent-encoded paths
/// need machinery the disk face lacks and return `None`.
fn missing_relative(uri: &DirectiveUri, dir: &Path, file: FileId) -> Option<Diagnostic> {
    let raw = parse_directive_uri(&uri.value).ok()?;
    if !raw.scheme.is_empty() || raw.authority.is_some() || raw.path.contains('%') {
        return None;
    }
    let target = normalize_lexical(&dir.join(&*raw.path));
    if target.exists() {
        return None;
    }
    Some(uri_diagnostic(
        missing_file_code(&target),
        &uri.value,
        uri.span,
        file,
    ))
}

/// The code for a known-path-but-missing target: generated-suffix paths
/// report `uri_has_not_been_generated`, everything else `uri_does_not_exist`
/// (the engine's `missing_file_code`; suffixes frozen at
/// `library_analyzer.dart:694-700`).
fn missing_file_code(path: &Path) -> ResolutionCode {
    const GENERATED_SUFFIXES: [&str; 6] = [
        ".g.dart",
        ".pb.dart",
        ".pbenum.dart",
        ".pbserver.dart",
        ".pbjson.dart",
        ".template.dart",
    ];
    let text = path.to_string_lossy();
    if GENERATED_SUFFIXES
        .iter()
        .any(|suffix| text.ends_with(suffix))
    {
        ResolutionCode::UriHasNotBeenGenerated
    } else {
        ResolutionCode::UriDoesNotExist
    }
}

/// One URI-layer diagnostic from the resolution registry's descriptor (the
/// engine's `uri_diagnostic` face): the family decides TYPE and severity,
/// and the oracle message template takes the URI as written.
fn uri_diagnostic(code: ResolutionCode, uri: &str, range: SourceRange, file: FileId) -> Diagnostic {
    let info = code.info();
    Diagnostic::new(
        info.dtype.severity(),
        CodeInfo::new(info.name, info.machine, info.dtype, info.has_published),
        info.message.replace("{0}", uri),
        file,
        range,
        info.correction.map(str::to_owned),
    )
}

/// Walks the directive region of a token table (the engine diet walk's
/// region rules: after an optional script tag, metadata, and `;`s,
/// `library`/`import`/`export`/`part` open directive shapes consumed to
/// their `;`; any other token ends the region) and returns the resolvable
/// URIs of `import`/`export`/`part`/`part of` string forms.
fn harvest_directive_uris(text: &str, tokens: &TokenTable) -> Vec<DirectiveUri> {
    let mut uris = Vec::new();
    let mut i = 0;
    while let Some(token) = tokens.get(i) {
        match token.kind {
            // EOF and any non-directive token end the region.
            TokenKind::Semicolon | TokenKind::ScriptTag => i += 1,
            TokenKind::At => i = skip_annotation(tokens, i + 1),
            TokenKind::Keyword(Keyword::Library) => i = skip_dotted_name(tokens, i + 1),
            TokenKind::Keyword(Keyword::Import | Keyword::Export) => {
                let (uri, end) = read_uri(text, tokens, i + 1);
                if let Some(uri) = uri {
                    uris.push(uri);
                }
                i = skip_to_semicolon(tokens, end);
            }
            TokenKind::Keyword(Keyword::Part) => i = harvest_part(text, tokens, i, &mut uris),
            _ => break,
        }
    }
    uris
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

/// A dotted name (`a.b.c`) of identifier-ish tokens, consumed to the `;`
/// (`library a.b.c;` / `part of a.b;` carry no URI).
fn skip_dotted_name(tokens: &TokenTable, mut i: usize) -> usize {
    loop {
        match kind_at(tokens, i) {
            Some(TokenKind::Semicolon) | None => return i + 1,
            Some(kind) if is_name_token(kind) || kind == TokenKind::Period => i += 1,
            _ => return i + 1,
        }
    }
}

/// Consumes the rest of an `import`/`export` shape to (and past) its `;`.
fn skip_to_semicolon(tokens: &TokenTable, mut i: usize) -> usize {
    loop {
        match kind_at(tokens, i) {
            None | Some(TokenKind::Eof) => return i,
            Some(TokenKind::Semicolon) => return i + 1,
            _ => i += 1,
        }
    }
}

/// Reads the URI literal starting at `i`: the run of adjacent plain `String`
/// segments concatenates into one value, and any interpolation segment makes
/// the region shapeless (the whole chain is skipped so the region walk stays
/// aligned). A cooked segment carrying escapes stays unjudged — its value
/// needs the engine's escape processor — while raw segments cook verbatim.
/// Returns the URI (absent when shapeless or unjudged) and the index just
/// past the literal region.
fn read_uri(text: &str, tokens: &TokenTable, i: usize) -> (Option<DirectiveUri>, usize) {
    if kind_at(tokens, i) != Some(TokenKind::String) {
        return (None, i);
    }
    let mut j = i;
    let mut value = String::new();
    let mut judged = true;
    let span_offset = tokens.get(i).expect("kind_at checked").utf16_offset;
    // Always overwritten by the first iteration (segment `i` is a `String`).
    let mut span_end = span_offset;
    while kind_at(tokens, j) == Some(TokenKind::String) {
        let lexeme = tokens.lexeme(text, j);
        let token = tokens.get(j).expect("kind_at checked");
        span_end = token.utf16_offset + utf16_len_of(lexeme);
        match simple_string_value(lexeme) {
            Some(segment) => value.push_str(segment),
            None => judged = false,
        }
        j += 1;
    }
    if matches!(
        kind_at(tokens, j),
        Some(TokenKind::StringInterpolationExpression | TokenKind::StringInterpolationIdentifier)
    ) {
        loop {
            match kind_at(tokens, j) {
                Some(TokenKind::String | TokenKind::StringInterpolationIdentifier) => j += 1,
                Some(TokenKind::StringInterpolationExpression) => {
                    j = tokens.matching_end(j).map_or(j + 1, |end| end + 1);
                }
                _ => break,
            }
        }
        return (None, j);
    }
    let uri = judged.then_some(DirectiveUri {
        value,
        span: SourceRange {
            offset: span_offset,
            length: span_end - span_offset,
        },
    });
    (uri, j)
}

/// `part 'u';` and `part of 'u' | a.b.c;` — the string forms carry a URI.
fn harvest_part(
    text: &str,
    tokens: &TokenTable,
    start: usize,
    out: &mut Vec<DirectiveUri>,
) -> usize {
    if kind_at(tokens, start + 1) == Some(TokenKind::Keyword(Keyword::Of)) {
        if kind_at(tokens, start + 2) == Some(TokenKind::String) {
            let (uri, mut i) = read_uri(text, tokens, start + 2);
            if let Some(uri) = uri {
                out.push(uri);
            }
            if kind_at(tokens, i) == Some(TokenKind::Semicolon) {
                i += 1;
            }
            return i;
        }
        return skip_dotted_name(tokens, start + 2);
    }
    let (uri, mut i) = read_uri(text, tokens, start + 1);
    if let Some(uri) = uri {
        out.push(uri);
    }
    if kind_at(tokens, i) == Some(TokenKind::Semicolon) {
        i += 1;
    }
    i
}

/// The literal's cooked value when that is certain without escape
/// processing: raw bodies are verbatim; cooked bodies are verbatim unless
/// they carry a backslash (then `None`). A `$` in a plain `String` token is
/// a literal dollar — the scanner splits interpolated segments into their
/// own token kinds.
fn simple_string_value(lexeme: &str) -> Option<&str> {
    let (raw, body) = match lexeme.strip_prefix('r') {
        Some(rest) => (true, rest),
        None => (false, lexeme),
    };
    let body = if let Some(rest) = body.strip_prefix("\"\"\"") {
        rest.strip_suffix("\"\"\"")?
    } else if let Some(rest) = body.strip_prefix("'''") {
        rest.strip_suffix("'''")?
    } else if let Some(rest) = body.strip_prefix('"') {
        rest.strip_suffix('"')?
    } else {
        body.strip_prefix('\'')?.strip_suffix('\'')?
    };
    if !raw && body.contains('\\') {
        return None;
    }
    Some(body)
}

/// Scans `source`, honoring the `@dart` override consumer contract (scanner
/// module docs; official `source_loader.dart:1194–1209` — the config is
/// re-derived and the file rescanned before parsing). An override above the
/// latest known version is dropped with the official `STATIC_WARNING`
/// (official scanner: `_overrideVersion = null`); the scanner reports only
/// the last header-window match, so the loop settles after at most one
/// rescan.
///
/// The same settlement the engine's `l1::scan_unit` runs; this restatement
/// is the M0 face's own (check has no engine involvement), and the index
/// face seeds it with the owning package's version.
fn scan_file(source: &str, file: FileId) -> (LanguageVersion, ScanResult, Vec<Diagnostic>) {
    scan_file_at(source, DEFAULT_LANGUAGE_VERSION, file)
}

/// [`scan_file`] seeded with the file's package-derived language version
/// (the engine capture's derivation) instead of the default.
pub(crate) fn scan_file_at(
    source: &str,
    seed: LanguageVersion,
    file: FileId,
) -> (LanguageVersion, ScanResult, Vec<Diagnostic>) {
    let mut version = seed;
    let mut diagnostics = Vec::new();
    loop {
        let features = FeatureSet::for_language_version(version);
        let result = scan(
            source,
            ScanConfig {
                enable_triple_shift: triple_shift_enabled(version),
                enable_augmentations: features.is_enabled(Feature::Augmentations),
            },
            file,
        );
        let Some(override_version) = result.language_version_override else {
            return (version, result, diagnostics);
        };
        // Both override rows pin at the version comment — the whole
        // `// @dart = x.y` lexeme, isomorphic to the official
        // `LanguageVersionToken` (`analyzer scanner.dart:124`
        // `.atOffset(versionToken.offset, versionToken.length)`; oracle
        // `dart analyze --format=machine` on a line-1 comment:
        // `1|1|15`).
        // Contract-drift defense, not a reachable path: the scanner
        // always records the comment span with an override — degrade
        // to the (0,0) anchor rather than crash the check face.
        let span = result
            .language_version_override_span
            .unwrap_or(SourceRange {
                offset: 0,
                length: 0,
            });
        if override_version.is_above_latest() {
            let code = CodeRegistry::builtin()
                .get("invalid_language_version_override")
                .expect("official registry pins invalid_language_version_override");
            // The warning drops the override (official scanner:
            // `_overrideVersion = null`) — the seed version stands.
            diagnostics.push(Diagnostic::new(
                Severity::Warning,
                code,
                format!(
                    "The language version override can't specify a version greater than \
                     the latest known language version: {DEFAULT_LANGUAGE_VERSION}."
                ),
                file,
                span,
                None,
            ));
            return (version, result, diagnostics);
        }
        // The floor is a verifier check, not a scanner drop
        // (`error_verifier.dart:5787-5809`): the override stays effective
        // and the ERROR rides beside the settled version. The gate reads
        // `== version` so the row is pushed exactly once — at settlement
        // (the loop rescans until the override matches the version in
        // force).
        if override_version.is_below_floor() && override_version == version {
            let code = CodeRegistry::builtin()
                .get("illegal_language_version_override")
                .expect("official registry pins illegal_language_version_override");
            diagnostics.push(Diagnostic::new(
                Severity::Error,
                code,
                format!("The language version must be {MIN_LANGUAGE_VERSION_CONSTRAINT}."),
                file,
                span,
                Some(
                    "Try removing the language version override and migrating the code.".to_owned(),
                ),
            ));
        }
        if override_version != version {
            version = override_version;
            continue;
        }
        return (version, result, diagnostics);
    }
}

/// Collects parser recoverable errors into [`Diagnostic`]s. Per the parser's
/// `message.rs` contract, the listener owns the translation into a
/// `dartq_diagnostics::Diagnostic`, including file attribution; event spans
/// arrive precomputed in the external UTF-16 convention ([`TokenSpan`]).
///
/// Event-coded experiment rows (`handle_experiment_not_enabled` — the
/// patterns/primary-constructors gates) ride the same translation as the
/// engine's `DiagCollector`: the bare flag renders through the parser's
/// `experiment_not_enabled_message` selection, severity [`Severity::Error`],
/// anchored at the report's begin token. The dot-shorthands gate needs no
/// entry here — it reports through `handle_recoverable_error` already.
struct DiagnosticListener {
    file: FileId,
    sink: Rc<RefCell<Vec<Diagnostic>>>,
}

impl ParserListener for DiagnosticListener {
    fn handle_recoverable_error(&mut self, message: Message, start: TokenSpan, end: TokenSpan) {
        let offset = start.range.offset;
        let length = end.range.end() - offset;
        self.sink.borrow_mut().push(Diagnostic::new(
            Severity::Error,
            message.code,
            message.text,
            self.file,
            SourceRange { offset, length },
            message.correction,
        ));
    }

    fn handle_experiment_not_enabled(
        &mut self,
        flag: ExperimentalFlag,
        begin: TokenSpan,
        _end: TokenSpan,
    ) {
        let message = experiment_not_enabled_message(flag);
        self.sink.borrow_mut().push(Diagnostic::new(
            Severity::Error,
            message.code,
            message.text,
            self.file,
            begin.range,
            message.correction,
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dartq_diagnostics::{DiagnosticType, EXPECTED_TOKEN};

    /// Byte-shape parity with the harness machine parser
    /// (the retired harness's machine reader `parse_machine`): 8 `|`-separated
    /// fields, uppercase severity/type/code, 1-based line/col, UTF-16 length.
    #[test]
    fn machine_line_parity_with_harness() {
        let mut set = FileSet::default();
        let file = set.push("void f() { x }\n", Path::new("/c/lib/a.dart"));
        // offset 11 (`x`), length 1 → line 1, column 12
        let diag = Diagnostic::new(
            Severity::Error,
            EXPECTED_TOKEN,
            "Expected to find ';'.",
            file,
            SourceRange {
                offset: 11,
                length: 1,
            },
            None,
        );
        let mut out = Vec::new();
        write_machine(
            &mut out,
            &AnalysisOutput {
                diagnostics: &[diag],
                files: &set,
            },
        )
        .expect("write");
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "ERROR|SYNTACTIC_ERROR|EXPECTED_TOKEN|/c/lib/a.dart|1|12|1|Expected to find ';'.\n"
        );
    }

    /// The verbosity tiers' row shapes (the bench sweep's tier axis):
    /// `default` is the pre-change oracle bytes — delegation to
    /// `write_machine`, pinned against the oracle writer's own output AND
    /// the literal row (the regression guard) — `concise` drops the message
    /// field, `verbose` appends the correctionMessage as the ninth field
    /// (`-` when the template carries none; `|` escapes like the message
    /// field).
    #[test]
    fn verbosity_tiers_pin_the_row_shapes() {
        let mut set = FileSet::default();
        let file = set.push("void f() { x }\n", Path::new("/c/lib/a.dart"));
        let diagnostics = [
            Diagnostic::new(
                Severity::Error,
                EXPECTED_TOKEN,
                "Expected to find ';'.",
                file,
                SourceRange {
                    offset: 11,
                    length: 1,
                },
                None,
            ),
            Diagnostic::new(
                Severity::Warning,
                CodeInfo::new(
                    "synthetic",
                    "SYNTHETIC",
                    DiagnosticType::StaticWarning,
                    false,
                ),
                "Some | message",
                file,
                SourceRange {
                    offset: 0,
                    length: 3,
                },
                Some("Try removing the | keyword.".to_owned()),
            ),
        ];
        let output = AnalysisOutput {
            diagnostics: &diagnostics,
            files: &set,
        };
        let render = |verbosity| {
            let mut out = Vec::new();
            write_machine_verbosity(&mut out, &output, verbosity).expect("write");
            String::from_utf8(out).unwrap()
        };

        // `default`: byte-identical to the oracle writer (the pre-change
        // output).
        let mut oracle = Vec::new();
        write_machine(&mut oracle, &output).expect("write");
        assert_eq!(
            render(DiagnosticVerbosity::Default),
            String::from_utf8(oracle).unwrap(),
            "the default tier is the oracle writer verbatim"
        );
        assert_eq!(
            render(DiagnosticVerbosity::Default),
            "ERROR|SYNTACTIC_ERROR|EXPECTED_TOKEN|/c/lib/a.dart|1|12|1|Expected to find ';'.\n\
             WARNING|STATIC_WARNING|SYNTHETIC|/c/lib/a.dart|1|1|3|Some \\| message\n"
        );

        // `concise`: the message field drops, the identifying prefix stays.
        assert_eq!(
            render(DiagnosticVerbosity::Concise),
            "ERROR|SYNTACTIC_ERROR|EXPECTED_TOKEN|/c/lib/a.dart|1|12|1\n\
             WARNING|STATIC_WARNING|SYNTHETIC|/c/lib/a.dart|1|1|3\n"
        );

        // `verbose`: the correctionMessage rides as the ninth field, `-`
        // when absent, `|`-escaped like every free-text field.
        assert_eq!(
            render(DiagnosticVerbosity::Verbose),
            "ERROR|SYNTACTIC_ERROR|EXPECTED_TOKEN|/c/lib/a.dart|1|12|1|Expected to find ';'.|-\n\
             WARNING|STATIC_WARNING|SYNTHETIC|/c/lib/a.dart|1|1|3|Some \\| message|\
             Try removing the \\| keyword.\n"
        );
    }

    /// The parser's registry-verbatim correctionMessage rides through the
    /// check face's listener (#727): the `base_enum` modifier row carries
    /// "Try removing the keyword 'base'." unmodified.
    #[test]
    fn parse_diagnostic_carries_the_registry_correction() {
        let mut set = FileSet::default();
        let file = set.push("base enum E { a }\n", Path::new("/c/lib/a.dart"));
        let diags = analyze_file("base enum E { a }\n", Path::new("/c/lib"), file);
        let diagnostic = diags
            .iter()
            .find(|d| d.code.name == "base_enum")
            .expect("the base modifier row fires");
        assert_eq!(diagnostic.message, "Enums can't be declared to be 'base'.");
        assert_eq!(
            diagnostic.correction.as_deref(),
            Some("Try removing the keyword 'base'."),
        );
    }

    /// The event-coded experiment gate renders on the check face (#934):
    /// a file pinned below a shipped feature's version takes the parser's
    /// bare `handle_experiment_not_enabled` payload (the
    /// primary-constructors gate at `// @dart = 2.12`), rendered through
    /// the same `experiment_not_enabled_message` selection the engine's
    /// `DiagCollector` uses. Oracle probes (dart 3.13.1, machine format)
    /// split the declaration kinds on `endPrimaryConstructor`
    /// (`ast_builder.dart:2855)`: the Class/Enum arm gates at the
    /// constructor's begin token, so `class C(int x);` at 2.12 is the
    /// two-row experiment face — `EXPERIMENT_NOT_ENABLED` on the `(` (`2|8|1`)
    /// plus parseClass's `;`-body row (`2|15|1`) — while the
    /// `TopLevel|Mixin|Extension` arm is silent, so `mixin M(int x);` takes
    /// only the parser rows: `UNEXPECTED_TOKEN` at the `(` (`2|8|1`) plus
    /// `EXPERIMENT_NOT_ENABLED` at the `;` (`2|15|1`, the parser event's
    /// anchor); at the current version `mixin_primary_constructor`
    /// replaces the gate (`1|8|1`). The patterns family never gates
    /// on this face either: the official parser short-circuits the
    /// condition's `case` branch on `isPatternsFeatureEnabled`
    /// (`parser_impl.dart:8416`), so the pinned if-case reports the
    /// `expected_token` syntax face at `case` (`3|9|4`) — no experiment
    /// row; the same probe's `non_bool_condition` row is semantic, absent
    /// on this scan+parse face by design.
    #[test]
    #[expect(clippy::cast_possible_truncation)] // span arithmetic narrows under the scanner u32 offset cap
    fn experiment_gate_renders_on_the_check_face() {
        // The Class/Enum arm: the `(`-anchored gate row plus the `;`-body
        // row, no syntax rows (the paired formals parse clean pre-feature).
        let pinned_class = "// @dart = 2.12\nclass C(int x);\n";
        let mut set = FileSet::default();
        let file = set.push(pinned_class, Path::new("/c/lib/d.dart"));
        let diags = analyze_file(pinned_class, Path::new("/c/lib"), file);
        let shape: Vec<(&str, u32, u32)> = diags
            .iter()
            .map(|d| (d.code.name, d.range.offset, d.range.length))
            .collect();
        assert_eq!(
            shape,
            vec![
                ("experiment_not_enabled", 23, 1),
                ("experiment_not_enabled", 30, 1)
            ],
            "the oracle-pinned pre-feature class face"
        );

        // The silent TopLevel|Mixin|Extension arm: parser rows only, no
        // `(`-anchored experiment row.
        let pinned = "// @dart = 2.12\nmixin M(int x);\n";
        let mut set = FileSet::default();
        let file = set.push(pinned, Path::new("/c/lib/a.dart"));
        let diags = analyze_file(pinned, Path::new("/c/lib"), file);
        let shape: Vec<(&str, u32, u32)> = diags
            .iter()
            .map(|d| (d.code.name, d.range.offset, d.range.length))
            .collect();
        assert_eq!(
            shape,
            vec![
                ("unexpected_token", 23, 1),
                ("experiment_not_enabled", 30, 1)
            ],
            "the oracle-pinned pre-feature mixin face"
        );
        let gate = diags
            .iter()
            .find(|d| d.code.name == "experiment_not_enabled")
            .expect("the gate row is pinned above");
        assert_eq!(gate.severity, Severity::Error);
        assert_eq!(
            gate.message,
            "This requires the 'primary-constructors' language feature to be enabled."
        );

        let current = "mixin N(int y);\n";
        let mut set = FileSet::default();
        let file = set.push(current, Path::new("/c/lib/b.dart"));
        let diags = analyze_file(current, Path::new("/c/lib"), file);
        let shape: Vec<(&str, u32, u32)> = diags
            .iter()
            .map(|d| (d.code.name, d.range.offset, d.range.length))
            .collect();
        assert_eq!(
            shape,
            vec![("mixin_primary_constructor", 7, 1)],
            "at the current version the gate is replaced by the mixin face"
        );

        // The patterns short-circuit: the pinned if-case takes the
        // parser's no-case branch — the gate face is the expected-token
        // row at `case`, never an experiment row.
        let pinned_if_case = "// @dart = 2.12\nvoid f(Object o) {\n  if (o case int x) {}\n}\n";
        let mut set = FileSet::default();
        let file = set.push(pinned_if_case, Path::new("/c/lib/c.dart"));
        let diags = analyze_file(pinned_if_case, Path::new("/c/lib"), file);
        assert!(
            diags
                .iter()
                .all(|d| d.code.name != "experiment_not_enabled"),
            "the parser short-circuits the case branch pre-patterns: {diags:?}"
        );
        let case_head = pinned_if_case.find("case").expect("case token") as u32;
        let syntax = diags
            .iter()
            .find(|d| d.code.name == "expected_token")
            .expect("the short-circuit face reports the missing `)`");
        assert_eq!(syntax.range.offset, case_head, "anchored at `case`");
        assert_eq!(syntax.range.length, 4, "the whole `case` lexeme");
    }

    /// The F11 override contract: differing override rescans; above-latest
    /// drops with the official warning; last header match wins.
    #[test]
    fn override_rescan_drop_and_last_wins() {
        let mut table = FileTable::new();
        let file = table.intern(Path::new("t.dart"));

        let (version, _, warnings) = scan_file("// @dart = 2.12\nvar x = 1;\n", file);
        assert_eq!(version, LanguageVersion::new(2, 12), "override adopted");
        assert!(warnings.is_empty());

        let (version, _, warnings) = scan_file("// @dart = 9.9\n", file);
        assert_eq!(version, DEFAULT_LANGUAGE_VERSION, "above-latest dropped");
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].code.name, "invalid_language_version_override");
        assert_eq!(warnings[0].severity, Severity::Warning);
        assert_eq!(warnings[0].range.offset, 0, "span at the comment");
        assert_eq!(warnings[0].range.length, 14, "whole comment lexeme");

        // Era anchors: 3.14
        // is above the oracle's latest known version — warned and dropped;
        // 3.13 is the latest and is accepted.
        let (version, _, warnings) = scan_file("// @dart = 3.14\nvar x = 1;\n", file);
        assert_eq!(
            version, DEFAULT_LANGUAGE_VERSION,
            "above-latest 3.14 dropped"
        );
        assert_eq!(warnings.len(), 1);

        let (version, _, warnings) = scan_file("// @dart = 3.13\nvar x = 1;\n", file);
        assert_eq!(
            version,
            LanguageVersion::new(3, 13),
            "oracle latest accepted"
        );
        assert!(warnings.is_empty());

        let (version, ..) = scan_file("// @dart = 2.12\n// @dart = 3.1\n", file);
        assert_eq!(version, LanguageVersion::new(3, 1), "last match wins");

        let (version, ..) = scan_file("var x = 1;\n", file);
        assert_eq!(version, DEFAULT_LANGUAGE_VERSION);
    }

    /// The official override floor (`analysis_options.dart:183-185`,
    /// `>= 2.12.0`; upstream test
    /// `illegal_language_version_override_test.dart:29-34`): 2.9 reports
    /// the `illegal_language_version_override` ERROR at column 1, length 14
    /// ("The language version must be >=2.12.0."), 2.12 (equal) and 2.14
    /// are clean, and a below-floor override STAYS effective (the settled
    /// version is the written one — a verifier row, not a scanner drop).
    #[test]
    fn override_floor_illegal_below_2_12() {
        let mut table = FileTable::new();
        let file = table.intern(Path::new("t.dart"));

        let (version, _, errors) = scan_file("// @dart = 2.9\nint a = 0;\n", file);
        assert_eq!(
            version,
            LanguageVersion::new(2, 9),
            "below-floor override stays effective"
        );
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].code.name, "illegal_language_version_override");
        assert_eq!(errors[0].severity, Severity::Error);
        assert_eq!(errors[0].range.offset, 0, "column 1");
        assert_eq!(errors[0].range.length, 14, "the whole comment lexeme");
        assert_eq!(errors[0].message, "The language version must be >=2.12.0.");
        assert_eq!(
            errors[0].correction.as_deref(),
            Some("Try removing the language version override and migrating the code.")
        );

        let (version, _, errors) = scan_file("// @dart = 2.11\nvar x = 1;\n", file);
        assert_eq!(version, LanguageVersion::new(2, 11));
        assert_eq!(errors.len(), 1, "2.11 is below the floor too");

        let (_, _, errors) = scan_file("// @dart = 2.12\nvar x = 1;\n", file);
        assert!(errors.is_empty(), "the floor itself is allowed");

        let (_, _, errors) = scan_file("// @dart = 2.14\nvar x = 1;\n", file);
        assert!(errors.is_empty());
    }

    /// The above-latest warning emits the oracle-matching machine line: the
    /// span is the whole `@dart` comment lexeme (oracle Dart 3.13.1,
    /// `dart analyze --format=machine` on `// @dart = 99.9`:
    /// `WARNING|STATIC_WARNING|INVALID_LANGUAGE_VERSION_OVERRIDE|<file>|1|1|15|…`;
    /// indented: `1|3|15`).
    #[test]
    fn override_warning_machine_span() {
        fn machine_line(source: &str, path: &str) -> String {
            let mut set = FileSet::default();
            let file = set.push(source, Path::new(path));
            let (_, _, diags) = scan_file(source, file);
            assert_eq!(diags.len(), 1);
            let mut out = Vec::new();
            write_machine(
                &mut out,
                &AnalysisOutput {
                    diagnostics: &diags,
                    files: &set,
                },
            )
            .expect("write");
            String::from_utf8(out).unwrap()
        }

        let expected = format!(
            "The language version override can't specify a version greater than \
             the latest known language version: {DEFAULT_LANGUAGE_VERSION}."
        );
        assert_eq!(
            machine_line("// @dart = 99.9\nvar x = 1;\n", "/c/lib/a.dart"),
            format!(
                "WARNING|STATIC_WARNING|INVALID_LANGUAGE_VERSION_OVERRIDE|/c/lib/a.dart|\
                 1|1|15|{expected}\n"
            )
        );
        assert_eq!(
            machine_line("  // @dart = 9.9\n", "/c/lib/b.dart"),
            format!(
                "WARNING|STATIC_WARNING|INVALID_LANGUAGE_VERSION_OVERRIDE|/c/lib/b.dart|\
                 1|3|14|{expected}\n"
            )
        );
    }

    /// The oracle display order reaches the machine rows: a file whose
    /// earliest row is the above-latest `invalid_language_version_override`
    /// WARNING (offset 0) plus a later parse ERROR emits the ERROR row
    /// first — severity leads the comparator (DiagnosticWithPath.compareTo,
    /// upstream Dart SDK source, pkg/dartdev/lib/src/commands/analyze.dart:
    /// 681-705). The retired check-face order (path-first with a code
    /// tiebreak) emitted the WARNING first.
    #[test]
    fn analyze_files_orders_severity_before_path() {
        let root = std::env::temp_dir().join(format!("dq_cmd_order_{}", std::process::id()));
        fs::create_dir_all(&root).expect("mkdir");
        fs::write(root.join("a.dart"), "// @dart = 99.9\nvoid f() { x }\n").expect("write");

        let files = BTreeSet::from([root.join("a.dart")]);
        let run = analyze_files(&files).expect("run");
        let diags = run.diagnostics();
        assert_eq!(diags.len(), 2, "the warning plus the parse error");
        assert_eq!(diags[0].code.name, "expected_token", "the ERROR row leads");
        assert_eq!(diags[0].severity, Severity::Error);
        assert_eq!(
            diags[1].code.name, "invalid_language_version_override",
            "the WARNING row follows despite its smaller offset"
        );

        let mut out = Vec::new();
        write_machine(&mut out, &run.output()).expect("write");
        let text = String::from_utf8(out).unwrap();
        let (error_row, warning_row) = text.split_once('\n').expect("two rows");
        assert!(error_row.starts_with("ERROR|"), "{error_row}");
        assert!(warning_row.starts_with("WARNING|"), "{warning_row}");

        fs::remove_dir_all(&root).ok();
    }

    /// The CLI read face strips exactly one leading BOM, matching the oracle
    /// read layer: single BOM gone, second BOM kept as content, BOM-only
    /// file reads back empty.
    #[test]
    fn read_source_strips_one_leading_bom() {
        let root = std::env::temp_dir().join(format!("dq_cmd_bom_{}", std::process::id()));
        fs::create_dir_all(&root).expect("mkdir");
        fs::write(root.join("single.dart"), "\u{FEFF}var x;").expect("write");
        fs::write(root.join("double.dart"), "\u{FEFF}\u{FEFF}x").expect("write");
        fs::write(root.join("only.dart"), "\u{FEFF}").expect("write");
        fs::write(root.join("plain.dart"), "var x;").expect("write");

        let (_, single) = read_source(&root.join("single.dart")).expect("read");
        assert_eq!(single, "var x;");
        let (_, double) = read_source(&root.join("double.dart")).expect("read");
        assert_eq!(double, "\u{FEFF}x");
        let (_, only) = read_source(&root.join("only.dart")).expect("read");
        assert!(only.is_empty(), "BOM-only file strips to the empty string");
        let (_, plain) = read_source(&root.join("plain.dart")).expect("read");
        assert_eq!(plain, "var x;");

        fs::remove_dir_all(&root).ok();
    }

    /// The span-capacity gate (#1543 bypass face): a file over
    /// [`MAX_SOURCE_BYTES`] is refused with the path and both sizes in the
    /// message. The sparse fixture claims the size with `set_len` — no
    /// blocks are materialized — and the stat rejects before any read. The
    /// accepted side of the boundary is the constant's own unit test in
    /// dartq-source; a physical `== MAX` fixture would need a 4 GiB read.
    #[test]
    fn read_source_rejects_span_capacity_files() {
        let root = std::env::temp_dir().join(format!("dq_cmd_capacity_{}", std::process::id()));
        fs::create_dir_all(&root).expect("mkdir");
        let path = root.join("huge.dart");
        fs::File::create(&path)
            .and_then(|file| file.set_len(MAX_SOURCE_BYTES as u64 + 1))
            .expect("sparse fixture");

        let error = read_source(&path).expect_err("over-capacity file rejected");
        let line = error.to_string();
        let canonical = path.canonicalize().expect("canonical");
        assert!(
            line.contains(canonical.to_str().expect("utf-8 temp path")),
            "{line}"
        );
        assert!(line.contains(&MAX_SOURCE_BYTES.to_string()), "{line}");
        assert!(
            line.contains(&(MAX_SOURCE_BYTES as u64 + 1).to_string()),
            "{line}"
        );

        fs::remove_dir_all(&root).ok();
    }

    /// The check face's posture for an over-capacity file: the rejection
    /// rides the read-failure channel — the run aborts with the error (a
    /// partial diagnostic set must never masquerade as a valid result),
    /// never silently skips the file.
    #[test]
    fn analyze_files_aborts_on_span_capacity_rejection() {
        let root = std::env::temp_dir().join(format!("dq_cmd_capacity_run_{}", std::process::id()));
        fs::create_dir_all(&root).expect("mkdir");
        fs::write(root.join("ok.dart"), "var ok = 1;\n").expect("write");
        fs::File::create(root.join("huge.dart"))
            .and_then(|file| file.set_len(MAX_SOURCE_BYTES as u64 + 1))
            .expect("sparse fixture");

        let files = crate::discover::discover(std::slice::from_ref(&root)).expect("discover");
        assert!(
            analyze_files(&files).is_err(),
            "the run aborts on the rejection"
        );

        fs::remove_dir_all(&root).ok();
    }

    /// The oracle read layer over an undecodable file
    /// (file_content_cache.dart:44-62): strict decode inside a catch-all —
    /// the file lands as empty content, contributes no diagnostics, and the
    /// run continues for its siblings. Before the pin, the decode failure
    /// aborted the whole run (`analyze_files` errored).
    #[test]
    fn analyze_files_survives_non_utf8_file() {
        use std::io::Write as _;

        let root = std::env::temp_dir().join(format!("dq_cmd_enc_{}", std::process::id()));
        fs::create_dir_all(&root).expect("mkdir");
        fs::write(root.join("broken.dart"), "void f() { x }\n").expect("write broken");
        // Deliberate invalid UTF-8 bytes inside a comment — the corpus
        // fixture blindspot_encoding.dart's shape (0xFF 0xFE).
        let mut file = fs::File::create(root.join("encoding.dart")).expect("create");
        file.write_all(b"// invalid: \xff\xfe\nvoid f() {}\n")
            .expect("write raw bytes");
        drop(file);

        let (_, source) = read_source(&root.join("encoding.dart")).expect("read");
        assert!(source.is_empty(), "undecodable file reads as empty content");

        let files = crate::discover::discover(std::slice::from_ref(&root)).expect("discover");
        let run = analyze_files(&files).expect("the run completes over the undecodable file");
        assert_eq!(run.file_count(), 2, "both files analyzed");
        assert_eq!(
            run.diagnostics().len(),
            1,
            "only broken.dart's syntax row; encoding.dart contributes none: {:?}",
            run.diagnostics()
        );
        assert_eq!(run.diagnostics()[0].code.name, "expected_token");

        fs::remove_dir_all(&root).ok();
    }

    /// The fallible parent resolution's happy face: the canonical file's
    /// directory (the composition root for relative directive targets).
    #[test]
    fn parent_dir_yields_the_canonical_parent() {
        let dir = parent_dir(Path::new("/c/lib/a.dart")).expect("parent");
        assert_eq!(dir, Path::new("/c/lib"));
    }

    /// The defensive degrade: a root path has no parent, and the miss is a
    /// structured error naming the path — never a panic. A root is never a
    /// file [`read_source`] accepts, so the arm is unreachable through
    /// [`analyze_files`]; the helper pins the contract directly.
    #[test]
    fn parent_dir_rejects_a_root_path_with_a_structured_error() {
        let error = parent_dir(Path::new("/")).expect_err("root has no parent");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("path has no parent"), "{error}");
    }

    /// End to end over a real temp tree: discovery, canonical paths, machine
    /// output, and the canonical diagnostic order.
    #[test]
    fn analyze_files_runs_pipeline_over_temp_tree() {
        let root = std::env::temp_dir().join(format!("dq_cmd_check_{}", std::process::id()));
        fs::create_dir_all(root.join("lib")).expect("mkdir");
        fs::write(root.join("lib/clean.dart"), "var ok = 1;\n").expect("write");
        fs::write(root.join("lib/broken.dart"), "void f() { x }\n").expect("write");

        let files = crate::discover::discover(std::slice::from_ref(&root)).expect("discover");
        let run = analyze_files(&files).expect("check core");
        assert_eq!(run.file_count(), 2);
        assert_eq!(run.loc(), 2);
        assert_eq!(run.diagnostics().len(), 1, "broken.dart's syntax error");
        assert_eq!(
            run.diagnostics()[0].code.name,
            "expected_token",
            "canonical order puts the one diagnostic forward"
        );

        fs::remove_dir_all(&root).ok();
    }

    /// The check face's analysis root (the lon/text locator's base): the
    /// directory the run was pointed at, so a one-target run locates its
    /// files under the target (`lib/broken.dart`, not `broken.dart` under
    /// the `lib/` root the file set alone would derive) — sibling targets
    /// climb to their covering parent, and a bare file list (no targets)
    /// keeps the deepest covering directory of the files.
    #[test]
    fn analyze_files_derives_the_analysis_root() {
        let root = std::env::temp_dir().join(format!("dq_cmd_root_{}", std::process::id()));
        fs::create_dir_all(root.join("lib")).expect("mkdir");
        fs::write(root.join("lib/broken.dart"), "void f() { x }\n").expect("write");

        let files = crate::discover::discover(std::slice::from_ref(&root)).expect("discover");
        let run = analyze_targets(std::slice::from_ref(&root), &files).expect("check core");
        let derived = run.output().files.analysis_root().expect("one root");
        assert_eq!(
            derived,
            root.canonicalize().expect("canonical").as_path(),
            "the target directory is the root"
        );
        let diag = &run.diagnostics()[0];
        let path = run.output().files.file_path(diag.file);
        assert_eq!(
            crate::pkg_label::locate(derived, path),
            "lib/broken.dart",
            "the row locates under the root"
        );
        let bare = analyze_files(&files).expect("check core");
        assert_eq!(
            bare.output().files.analysis_root(),
            Some(
                root.canonicalize()
                    .expect("canonical")
                    .join("lib")
                    .as_path()
            ),
            "no targets: the deepest covering directory of the files"
        );

        // Two sibling targets: the root climbs to their common parent —
        // the covering directory of the targets, not the first target.
        let other = std::env::temp_dir().join(format!("dq_cmd_root2_{}", std::process::id()));
        fs::create_dir_all(&other).expect("mkdir");
        fs::write(other.join("b.dart"), "void g() { y }\n").expect("write");
        let mut siblings = files;
        siblings.insert(other.join("b.dart"));
        let run = analyze_targets(&[root.clone(), other.clone()], &siblings).expect("check core");
        // Canonical spelling throughout: macOS temp dirs live behind the
        // `/var` → `/private/var` symlink, and the root follows the files.
        let canonical_temp = std::env::temp_dir().canonicalize().expect("canonical temp");
        assert_eq!(
            run.output().files.analysis_root(),
            Some(canonical_temp.as_path()),
            "the common parent covers both sibling targets"
        );

        fs::remove_dir_all(&root).ok();
        fs::remove_dir_all(&other).ok();
    }

    /// The disk pipeline's URI face (the row the CLI check face and the
    /// daemon's disk view both render — equivalence): a missing
    /// relative import surfaces `uri_does_not_exist` at the URI literal,
    /// `../` composes through the canonical parent, and the generated
    /// suffixes code `uri_has_not_been_generated`. Everything the face
    /// cannot judge stays silent: the satisfied import, the raw-string
    /// spelling of a present target, the interpolated and percent-encoded
    /// URIs, and the scheme-carrying `dart:`/`package:` imports (the
    /// engine/compat faces' universe).
    #[test]
    fn analyze_files_reports_missing_relative_imports() {
        let root = std::env::temp_dir().join(format!("dq_cmd_uri_{}", std::process::id()));
        fs::create_dir_all(root.join("lib/sub")).expect("mkdir");
        fs::write(
            root.join("lib/main.dart"),
            "import 'missing.dart';\nimport 'present.dart';\nvoid main() {}\n",
        )
        .expect("write main");
        fs::write(root.join("lib/present.dart"), "var ok = 1;\n").expect("write present");
        fs::write(
            root.join("lib/foreign.dart"),
            "import 'dart:core';\nimport 'package:nowhere/x.dart';\n",
        )
        .expect("write foreign");
        fs::write(
            root.join("lib/gen.dart"),
            "import 'missing.g.dart';\nimport r'present.dart';\n",
        )
        .expect("write gen");
        fs::write(
            root.join("lib/sub/deep.dart"),
            "import '../present.dart';\nimport 'gone.dart';\n",
        )
        .expect("write deep");
        fs::write(
            root.join("lib/unjudged.dart"),
            "import '${dynamic}.dart';\nimport 'a%20b.dart';\nvoid f() { x }\n",
        )
        .expect("write unjudged");

        let files = crate::discover::discover(std::slice::from_ref(&root)).expect("discover");
        let run = analyze_files(&files).expect("check core");

        let by_code = |name: &str| {
            run.diagnostics()
                .iter()
                .filter(|d| d.code.name == name)
                .collect::<Vec<_>>()
        };
        let missing = by_code("uri_does_not_exist");
        assert_eq!(
            missing.len(),
            2,
            "main.dart + sub/deep.dart only: {:?}",
            run.diagnostics()
        );
        assert_eq!(missing[0].severity, Severity::Error);
        assert_eq!(
            missing[0].message,
            "Target of URI doesn't exist: 'missing.dart'."
        );
        assert_eq!(
            missing[0].range.offset, 7,
            "the URI literal, quotes included"
        );
        assert_eq!(missing[0].range.length, 14, "'missing.dart' with quotes");
        assert!(
            run.output()
                .files
                .file_path(missing[1].file)
                .ends_with("lib/sub/deep.dart"),
            "`../` composed from the sub directory"
        );

        let generated = by_code("uri_has_not_been_generated");
        assert_eq!(
            generated.len(),
            1,
            "the .g.dart suffix codes the generated face"
        );

        // The syntax error in unjudged.dart is the only non-URI row; the
        // satisfied import, the raw-string spelling of a present target, the
        // interpolated and percent-encoded URIs, and the scheme-carrying
        // `dart:`/`package:` imports all stay silent.
        assert_eq!(run.diagnostics().len(), 4);

        fs::remove_dir_all(&root).ok();
    }

    /// The canonical order leads on the printed path's bytes, not the
    /// intern slot (#558): the symlink `a.dart → z.dart` dedups `z.dart`
    /// into the first intern slot (raw set order a, b, z), yet `b.dart`
    /// must sort before it by path bytes. Under the retired slot-id key
    /// the rows emitted z-before-b — the machine face's byte-parity
    /// contract broken.
    #[cfg(unix)]
    #[test]
    fn canonical_order_sorts_by_printed_path_under_symlink_dedup() {
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir().join(format!("dq_cmd_order_{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("mkdir");
        fs::write(root.join("z.dart"), "void f() { x }\n").expect("write z");
        fs::write(root.join("b.dart"), "void g() { y }\n").expect("write b");
        symlink(root.join("z.dart"), root.join("a.dart")).expect("link");

        let files = crate::discover::discover(std::slice::from_ref(&root)).expect("discover");
        let run = analyze_files(&files).expect("check core");
        let paths: Vec<&str> = run
            .diagnostics()
            .iter()
            .map(|d| {
                run.output()
                    .files
                    .file_path(d.file)
                    .to_str()
                    .expect("utf-8 fixture path")
            })
            .collect();
        assert_eq!(paths.len(), 2, "both real files analyzed: {paths:?}");
        let b_at = paths
            .iter()
            .position(|p| p.ends_with("b.dart"))
            .expect("b row");
        let z_at = paths
            .iter()
            .position(|p| p.ends_with("z.dart"))
            .expect("z row");
        assert!(
            b_at < z_at,
            "path-byte order: b.dart before z.dart, got {paths:?}"
        );

        fs::remove_dir_all(&root).ok();
    }

    /// Canonical dedup over the discovery set (context_root.dart:119-129):
    /// one file reachable under two component-distinct spellings — the
    /// `dartq check lib lib/../lib/a.dart` double-mention shape, where the
    /// explicit arg carries a `..` segment the walked entry lacks — is
    /// analyzed and emitted once. (A `./`-carrying spelling compares EQUAL
    /// component-wise to the plain one — Path/PathBuf equality and ordering
    /// are component-wise — so `.` spellings never enter the set as
    /// duplicates in the first place.) Before the dedup the second pass
    /// tripped the `FileSet` index invariant in debug builds and doubled the
    /// rows in release.
    #[test]
    fn analyze_files_dedupes_duplicate_spellings() {
        let root = std::env::temp_dir().join(format!("dq_cmd_dup_{}", std::process::id()));
        fs::create_dir_all(root.join("lib")).expect("mkdir");
        fs::write(root.join("lib/broken.dart"), "void f() { x }\n").expect("write");

        let mut files = crate::discover::discover(std::slice::from_ref(&root)).expect("discover");
        // A second spelling of the same file: the component vectors differ
        // (`..` is kept by Path comparison), the disk file is one.
        files.insert(root.join("lib/../lib/broken.dart"));
        assert_eq!(files.len(), 2, "the raw spellings both sit in the set");

        let run = analyze_files(&files).expect("check core");
        assert_eq!(run.file_count(), 1, "one file analyzed");
        assert_eq!(run.diagnostics().len(), 1, "one emission, not two");

        fs::remove_dir_all(&root).ok();
    }

    /// The symlink half of the dedup: a symlinked `.dart` file whose target
    /// is also walked (the discovery set legitimately carries both raw paths
    /// — the oracle's walk includes linked files) resolves to one canonical
    /// id. The fixture mirrors the CLI's
    /// `discover_includes_symlinked_files_not_symlinked_dirs` tree.
    #[cfg(unix)]
    #[test]
    fn analyze_files_dedupes_symlinked_duplicates() {
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir().join(format!("dq_cmd_symlink_{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("mkdir");
        fs::write(root.join("a.dart"), "void f() { x }\n").expect("write");
        symlink(root.join("a.dart"), root.join("b.dart")).expect("link");

        let files = crate::discover::discover(std::slice::from_ref(&root)).expect("discover");
        assert_eq!(
            files,
            BTreeSet::from([root.join("a.dart"), root.join("b.dart")]),
            "the walk carries both raw paths (linked files are analyzed)"
        );

        let run = analyze_files(&files).expect("check core");
        assert_eq!(run.file_count(), 1, "one canonical id behind both links");
        assert_eq!(run.diagnostics().len(), 1, "one emission");
        assert!(
            run.output()
                .files
                .file_path(run.diagnostics()[0].file)
                .ends_with("a.dart"),
            "the canonical target's spelling is the printed one"
        );

        fs::remove_dir_all(&root).ok();
    }
}
