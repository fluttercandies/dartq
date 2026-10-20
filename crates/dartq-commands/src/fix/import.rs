//! The import-synthesis family — dartq's equivalent of upstream's
//! `ImportLibrary` producers (upstream Dart SDK source,
//! pkg/analysis_server/lib/src/services/correction/dart/import_library.dart;
//! fix kinds `dart.fix.import.library*`, registration
//! pkg/analysis_server/lib/src/services/correction/fix_internal.dart).
//! An unresolved-name diagnostic whose miss name the maintained index can
//! attribute to a top-level declaration in another library produces one
//! patch per candidate library: `import '<uri>';` inserted at the
//! upstream `_addLibraryImports` position (upstream Dart SDK source,
//! pkg/analyzer_plugin/lib/src/utilities/change_builder/change_builder_dart.dart
//! — the directive-region group ordering and blank-line rules replicated
//! over the token stream).
//!
//! Two upstream legs are deliberately out: extension-member candidates
//! (`_namesForExtensionMember` needs a resolved target type the index does
//! not carry) and show-combinator narrowing (`ImportLibrary...Show` — the
//! dispatch's recorded decision: every candidate library is offered
//! plain). `dart:` candidates sort first per the upstream fix-kind
//! priority order (`librarySdk` before `libraryProject1..4`); a `part of`
//! unit (a library part cannot import) yields no rows.

use super::*;

/// The arm mask bits: which declaration faces a miss code's candidates
/// admit (the top-level kinds, the SDK library faces).
const ARM_TYPE: u8 = 1;
const ARM_CALLABLE: u8 = 2;
const ARM_EXTENSION: u8 = 4;
const ARM_SDK: u8 = 8;

/// The import-synthesis arm of an emitted semantic code, or `None` for a
/// code with no import producer. Only the legs a bare name lookup can
/// serve are wired — upstream's per-code registration (fix_internal.dart)
/// minus the member-targeted legs the index cannot prove
/// (`undefined_setter`/`undefined_operator`'s `forExtensionMember`-only
/// registrations, the `unchecked_*_of_nullable_value` legs,
/// `undefined_prefixed_name`'s prefix-dart lookup) and the codes dartq
/// never emits.
#[must_use]
pub(crate) fn import_arm(code: &str) -> Option<u8> {
    let mask = match code {
        // forTypeOrMember legs: class/mixin/enum/extension-type/typedef
        // names at type positions.
        "cast_to_non_type"
        | "extends_non_class"
        | "implements_non_class"
        | "mixin_of_non_class"
        | "mixin_with_non_class_superclass"
        | "non_type_in_catch_clause"
        | "non_type_as_type_argument"
        | "not_a_type"
        | "type_test_with_undefined_name"
        | "undefined_class" => ARM_TYPE | ARM_SDK,
        // The full bare-name legs: types, top-level callables/variables,
        // and named extensions (forTypeOrMember + forFunction +
        // forExtension registrations on these codes).
        "undefined_function" | "undefined_identifier" | "undefined_method"
        | "undefined_getter" => ARM_TYPE | ARM_CALLABLE | ARM_EXTENSION | ARM_SDK,
        _ => return None,
    };
    Some(mask)
}

/// Whether a [`DeclKind`] admits the arm's mask — top-level faces only;
/// member kinds never reach this (the harvest filters
/// `container.is_none()` first).
fn kind_admitted(mask: u8, kind: dartq_engine::DeclKind) -> bool {
    use dartq_engine::DeclKind;
    match kind {
        DeclKind::Class
        | DeclKind::Enum
        | DeclKind::Mixin
        | DeclKind::ExtensionType
        | DeclKind::TypeAlias => mask & ARM_TYPE != 0,
        DeclKind::Extension => mask & ARM_EXTENSION != 0,
        DeclKind::TopLevelFn | DeclKind::TopLevelVar | DeclKind::Getter | DeclKind::Setter => {
            mask & ARM_CALLABLE != 0
        }
        DeclKind::Method | DeclKind::Field | DeclKind::TypeParam => false,
    }
}

/// One candidate library for a miss name: the import URI to insert.
/// `sdk` marks `dart:` rows (sort-first per the upstream fix-kind order).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ImportCandidate {
    pub uri: String,
    pub sdk: bool,
}

/// One file's harvest: miss name → its candidate libraries, upstream
/// priority order (`dart:` first, then `package:`, then `file:` — the
/// registry's `librarySdk` → `libraryProject1..4` sequence).
#[derive(Debug, Default)]
pub(crate) struct ImportContext {
    /// Miss spelling → candidates, one entry per name per file.
    pub names: FxHashMap<Box<str>, Vec<ImportCandidate>>,
}

/// The canonical import URI of a decl file: `package:<name>/<lib-rel>`
/// under a package lib dir (the same projection the query/explain faces
/// use), `file://` with the encoded absolute path everywhere else — the
/// oracle's canonical-URI rule (upstream `PackageMapUriResolver.pathToUri`;
/// `dartq_resolution`'s `compose_canonical_uri` composes the identical
/// pair but takes that crate's own `PackageTable`, unreachable here).
fn import_uri(engine: &dartq_engine::Engine, path: &Path) -> String {
    let label = crate::pkg_label::pkg_label(engine.packages(), path);
    if label.starts_with("package:") {
        return label;
    }
    let encoded = dartq_resolution::uri::encode_uri_path(&label);
    if encoded.starts_with('/') {
        format!("file://{encoded}")
    } else {
        format!("file:///{encoded}")
    }
}

/// Harvests one target file's import candidates while the engine pass is
/// alive. A `part of` unit answers `None` (a library part cannot carry
/// imports — upstream's producer exits on `partOfDirective`). The
/// already-imported set is the file's resolved import targets plus its
/// `dart:` spellings, so a candidate the file already imports — under any
/// spelling — never re-offers.
pub(crate) fn import_contexts(
    engine: &dartq_engine::Engine,
    file: FileId,
    rows: &[Diagnostic],
    text: &str,
    fuse: &PlannerFuse,
) -> Option<ImportContext> {
    let facts = engine.file_facts(file)?;
    if facts
        .directives
        .iter()
        .any(|d| d.kind == dartq_engine::DirectiveKind::PartOf)
    {
        return None;
    }
    let mut imported_files: FxHashSet<FileId> = FxHashSet::default();
    let mut imported_dart: FxHashSet<&str> = FxHashSet::default();
    for directive in &facts.directives {
        if directive.kind != dartq_engine::DirectiveKind::Import {
            continue;
        }
        match directive.target {
            dartq_resolution::ResolvedTarget::Resolved(target) => {
                imported_files.insert(target);
            }
            dartq_resolution::ResolvedTarget::External => {
                if let Some(uri) = directive.uri.as_deref() {
                    imported_dart.insert(uri);
                }
            }
            dartq_resolution::ResolvedTarget::Missing => {}
        }
    }
    let mut context = ImportContext::default();
    for diagnostic in rows {
        if !fuse.step() {
            break;
        }
        let Some(mask) = import_arm(diagnostic.code.name) else {
            continue;
        };
        let range = diagnostic.range.offset..diagnostic.range.offset + diagnostic.range.length;
        let Some(name) = slice16(text, &range).filter(|slice| word_shape(slice)) else {
            continue;
        };
        if context.names.contains_key(name) {
            continue;
        }
        let mut candidates: Vec<ImportCandidate> = Vec::new();
        let mut seen_files: FxHashSet<FileId> = FxHashSet::default();
        for decl in &engine.query_symbols(name).decls {
            if decl.container.is_some()
                || decl.file == file
                || !kind_admitted(mask, decl.kind)
                || imported_files.contains(&decl.file)
                || !seen_files.insert(decl.file)
            {
                continue;
            }
            let Some(path) = engine.file_table().path(decl.file) else {
                continue;
            };
            candidates.push(ImportCandidate {
                uri: import_uri(engine, path),
                sdk: false,
            });
        }
        if mask & ARM_SDK != 0 {
            for (library, names) in dartq_engine::sdk_faces_gen::SDK_LIBRARY_FACES {
                let uri = format!("dart:{library}");
                if names.contains(&name) && !imported_dart.contains(uri.as_str()) {
                    candidates.push(ImportCandidate { uri, sdk: true });
                }
            }
        }
        if candidates.is_empty() {
            continue;
        }
        // Upstream's fix-kind order: dart: first, then package:, then the
        // rest; a stable sort keeps each group's query order.
        candidates.sort_by_key(|candidate| {
            if candidate.sdk {
                0
            } else if candidate.uri.starts_with("package:") {
                1
            } else {
                2
            }
        });
        context.names.insert(name.into(), candidates);
    }
    Some(context)
}

/// The per-file stage: one anchored `import '<uri>';` row per candidate
/// per arm diagnostic, deduped on the emitted `(offset, uri)` pair — two
/// diagnostics over the same miss name offer the same import once.
#[must_use]
pub(crate) fn from_imports(
    diagnostics: &[Diagnostic],
    context: Option<&ImportContext>,
    text: &str,
    tokens: &TokenTable,
    lines: &LineIndex,
    fuse: &PlannerFuse,
) -> Vec<RuleFix> {
    let Some(context) = context else {
        return Vec::new();
    };
    let mut rows = Vec::new();
    let mut offered: FxHashSet<(u32, &str)> = FxHashSet::default();
    for diagnostic in diagnostics {
        if !fuse.step() {
            break;
        }
        if import_arm(diagnostic.code.name).is_none() {
            continue;
        }
        let range = diagnostic.range.offset..diagnostic.range.offset + diagnostic.range.length;
        let Some(name) = slice16(text, &range) else {
            continue;
        };
        let Some(candidates) = context.names.get(name) else {
            continue;
        };
        for candidate in candidates {
            if !offered.insert((diagnostic.range.offset, candidate.uri.as_str())) {
                continue;
            }
            let Some((offset, length, replacement)) = import_edit(text, tokens, &candidate.uri)
            else {
                continue;
            };
            rows.push(RuleFix {
                rule: diagnostic.code.name,
                message: format!("Add 'import' of '{}'", candidate.uri),
                source: BUILTIN_SEMANTIC_SOURCE,
                edit: anchored_edit(text, lines, offset, length, &replacement),
                target: None,
            });
        }
    }
    rows
}

// ---------------------------------------------------------------------------
// Placement — _addLibraryImports replicated over the token stream
// ---------------------------------------------------------------------------

/// One scanned directive: byte extents plus its URI literal span (the
/// string forms only — `library`/`part of` carry none).
#[derive(Debug)]
struct DirectiveSpan {
    /// Byte offset of the directive's leading token.
    start: usize,
    /// Byte offset just past the directive's `;`.
    end: usize,
    /// The URI literal's byte span, quotes included (import/export/part).
    uri: Option<(usize, usize)>,
    kind: DirectiveClass,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DirectiveClass {
    Import,
    Export,
    Part,
    PartOf,
    Library,
}

fn token_kind(slice: &[dartq_scanner::Token], i: usize) -> Option<TokenKind> {
    slice.get(i).map(|token| token.kind)
}

/// Walks the directive region — after an optional script tag, metadata,
/// and stray `;`s, the `library`/`import`/`export`/`part` keyword-led
/// shapes consumed to their `;`; anything else ends the region — and
/// records each directive's extents. (The engine diet walk's region
/// rules; check.rs's `harvest_directive_uris` walks the same shape.)
fn directive_spans(tokens: &TokenTable) -> Vec<DirectiveSpan> {
    let slice = tokens.as_slice();
    let mut spans = Vec::new();
    // Byte offset past the leading comment region — upstream inserts at
    // `firstTokenAfterCommentAndMetadata.offset` for the first directive;
    // the token offsets already sit past comments, so the first
    // keyword/`@`'s own offset serves (a file-level comment stays above).
    // For a metadata-led directive (`@meta\nimport …`) upstream's
    // `directive.offset` is the annotation's own offset — the span start
    // records it so a before-insert lands above the metadata too.
    let mut i = 0;
    // The pending metadata lead: a `@`-run's start carries into the
    // directive that follows it (upstream's `directive.offset` is the
    // annotation's own offset — insertions before a directive land above
    // its metadata).
    let mut pending: Option<usize> = None;
    while let Some(token) = slice.get(i) {
        let start = pending.take().unwrap_or(token.byte_offset as usize);
        match token.kind {
            TokenKind::ScriptTag | TokenKind::Semicolon => i += 1,
            TokenKind::At => {
                pending = Some(token.byte_offset as usize);
                i = skip_metadata(tokens, i + 1);
            }
            TokenKind::Keyword(Keyword::Library) => {
                let (end, next) = directive_end(slice, i);
                spans.push(DirectiveSpan {
                    start,
                    end,
                    uri: None,
                    kind: DirectiveClass::Library,
                });
                i = next;
            }
            TokenKind::Keyword(Keyword::Import | Keyword::Export) => {
                let (uri, end, next) = uri_directive(slice, i + 1);
                spans.push(DirectiveSpan {
                    start,
                    end,
                    uri,
                    kind: if token.kind == TokenKind::Keyword(Keyword::Import) {
                        DirectiveClass::Import
                    } else {
                        DirectiveClass::Export
                    },
                });
                i = next;
            }
            TokenKind::Keyword(Keyword::Part) => {
                if token_kind(slice, i + 1) == Some(TokenKind::Keyword(Keyword::Of)) {
                    let (end, next) = directive_end(slice, i);
                    spans.push(DirectiveSpan {
                        start,
                        end,
                        uri: None,
                        kind: DirectiveClass::PartOf,
                    });
                    i = next;
                } else {
                    let (uri, end, next) = uri_directive(slice, i + 1);
                    spans.push(DirectiveSpan {
                        start,
                        end,
                        uri,
                        kind: DirectiveClass::Part,
                    });
                    i = next;
                }
            }
            _ => break,
        }
    }
    spans
}

/// Skips `@name[.name][(args)]` metadata starting after the `@`
/// (check.rs's `skip_annotation` shape: name/period runs plus one
/// balanced paren group).
fn skip_metadata(tokens: &TokenTable, mut i: usize) -> usize {
    let slice = tokens.as_slice();
    loop {
        match token_kind(slice, i) {
            Some(TokenKind::Identifier | TokenKind::Keyword(_) | TokenKind::Period) => i += 1,
            Some(TokenKind::OpenParen) => {
                i = tokens.matching_end(i).map_or(i, |end| end + 1);
            }
            _ => return i,
        }
    }
}

/// Consumes a directive to (and past) its `;`, answering
/// `(directive-end-byte-offset, next-index)`. A missing `;` (recovery —
/// a truncated directive region the scanner still tokenized) ends at the
/// last consumed token's end.
fn directive_end(slice: &[dartq_scanner::Token], start: usize) -> (usize, usize) {
    let mut end = slice
        .get(start)
        .map_or(0, |t| (t.byte_offset + t.byte_len) as usize);
    let mut i = start;
    while let Some(token) = slice.get(i) {
        end = (token.byte_offset + token.byte_len) as usize;
        i += 1;
        if token.kind == TokenKind::Semicolon || token.kind == TokenKind::Eof {
            break;
        }
    }
    (end, i)
}

/// A string-form directive (`import 'u' …;`/`export 'u' …;`/`part 'u';`):
/// the primary URI literal's byte span plus the directive end. A
/// conditional-import `if (…) 'uri'` clause's string never becomes the
/// primary — only the first String token counts.
fn uri_directive(
    slice: &[dartq_scanner::Token],
    mut i: usize,
) -> (Option<(usize, usize)>, usize, usize) {
    let mut uri = None;
    let mut end = 0;
    while let Some(token) = slice.get(i) {
        end = (token.byte_offset + token.byte_len) as usize;
        if token.kind == TokenKind::String && uri.is_none() {
            uri = Some((
                token.byte_offset as usize,
                (token.byte_offset + token.byte_len) as usize,
            ));
        }
        i += 1;
        if token.kind == TokenKind::Semicolon || token.kind == TokenKind::Eof {
            break;
        }
    }
    (uri, end, i)
}

/// The URI literal's inner text (quotes stripped) for classification.
fn uri_text<'s>(text: &'s str, span: &DirectiveSpan) -> &'s str {
    match span.uri {
        Some((a, b)) => text[a..b].trim_matches(['\'', '"']),
        None => "",
    }
}

/// The upstream `_addLibraryImports` placement, replicated over the
/// directive scan: between the existing imports in `dart:` → `package:` →
/// other order (sorted within each group, a blank line at a group
/// boundary), else after `library`, before the first `export`, before
/// the first `part`, or at the unit's first declaration / EOF. Answers
/// `(offset16, length16, replacement)` — always a pure insertion.
pub(crate) fn import_edit(
    text: &str,
    tokens: &TokenTable,
    uri: &str,
) -> Option<(u32, u32, String)> {
    let spans = directive_spans(tokens);
    if spans.iter().any(|span| span.kind == DirectiveClass::PartOf) {
        return None;
    }
    let is_dart = uri.starts_with("dart:");
    let is_package = uri.starts_with("package:");
    let line = format!("import '{uri}';\n");

    let imports: Vec<&DirectiveSpan> = spans
        .iter()
        .filter(|span| span.kind == DirectiveClass::Import)
        .collect();
    if !imports.is_empty() {
        let mut last_dart = false;
        let mut last_package = false;
        for existing in &imports {
            let existing_uri = uri_text(text, existing);
            let existing_dart = existing_uri.starts_with("dart:");
            let existing_package = existing_uri.starts_with("package:");
            let existing_relative = !existing_uri.contains(':');
            let new_before = uri < existing_uri;
            // The group insertions (upstream's verbatim decision order):
            // the span's `start` is the keyword offset — for the first
            // directive upstream backs up to `firstTokenAfterCommentAndMetadata`,
            // and the token offset already sits past comment trivia, so
            // the same offset serves; metadata on the first directive is
            // a file-level annotation the insertion stays under by luck of
            // the `@` being the recorded start.
            if is_dart && (!existing_dart || new_before) {
                let mut replacement = line.clone();
                if !existing_dart {
                    replacement.push('\n');
                }
                return Some(insert_at(text, existing.start, &replacement));
            }
            if is_package && (existing_relative || new_before) {
                let mut replacement = line.clone();
                if existing_relative {
                    replacement.push('\n');
                }
                return Some(insert_at(text, existing.start, &replacement));
            }
            if !is_dart && !is_package && !existing_dart && !existing_package && new_before {
                return Some(insert_at(text, existing.start, &line));
            }
            // `isLastExistingDart`/`isLastExistingPackage` — the last
            // import's family flags feed the append branch's blank-line
            // rule below.
            last_dart = existing_dart;
            last_package = existing_package;
        }
        // Appended after the last import; a package after a dart tail, or
        // a non-dart after a dart/package tail, takes the blank line
        // (upstream's `trailingNewLine` rule at the append).
        let last = imports.last().expect("non-empty imports");
        let mut replacement = String::from("\n");
        if (is_package && last_dart) || (!is_dart && !is_package && (last_dart || last_package)) {
            replacement.push('\n');
        }
        replacement.push_str(line.trim_end());
        return Some(insert_at(text, last.end, &replacement));
    }
    if let Some(library) = spans
        .iter()
        .find(|span| span.kind == DirectiveClass::Library)
    {
        return Some(insert_at(
            text,
            library.end,
            &format!("\n\n{}", line.trim_end()),
        ));
    }
    if let Some(export) = spans
        .iter()
        .find(|span| span.kind == DirectiveClass::Export)
    {
        let mut replacement = line.clone();
        replacement.push('\n');
        return Some(insert_at(text, export.start, &replacement));
    }
    if let Some(part) = spans
        .iter()
        .find(|span| span.kind == DirectiveClass::Part)
    {
        let mut replacement = line.clone();
        replacement.push('\n');
        return Some(insert_at(text, part.start, &replacement));
    }
    // No directives: before the first declaration (a blank line after, the
    // upstream `insertEmptyLineAfter`), or at the unit's end.
    let first = tokens
        .as_slice()
        .iter()
        .find(|token| token.kind != TokenKind::Eof);
    match first {
        Some(token) => {
            let mut replacement = line.clone();
            replacement.push('\n');
            Some(insert_at(text, token.byte_offset as usize, &replacement))
        }
        None => Some(insert_at(text, text.len(), &line)),
    }
}

/// `(offset16, 0, replacement)` — the insertion edit over the UTF-16
/// offset the anchor layer's conversion supplies.
fn insert_at(text: &str, byte_offset: usize, replacement: &str) -> (u32, u32, String) {
    (
        byte_offset_to_utf16(text, byte_offset),
        0,
        replacement.to_owned(),
    )
}
