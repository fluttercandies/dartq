use super::{
    Check, NodeKind, PayloadField, RuleSpec, container_members, is_deprecated_metadata, narrow_u32,
    top_level_annotations,
};
use crate::rules::initialized_identifier_name;
use std::path::{Path, PathBuf};

/// Official: `pkg/linter/lib/src/rules/deprecated_member_use_from_same_package.dart`
/// — a use of a deprecated element from inside the declaring package
/// reports at the use. The official mount is one
/// `ElementUsageFrontierVisitor` per compilation unit (:102-113): every
/// declaration is pushed onto the frontier stack while its contents are
/// visited (`element_usage_frontier_visitor.dart`), so a use inside a
/// deprecated ancestor — the deprecated `library` directive, a
/// deprecated class/member/parameter — never reports (the
/// `test_deprecatedClass_usedInDeprecated*` family). Two message
/// variants under one name: the normalized `@Deprecated` tag with the
/// message, a bare `@deprecated` without.
///
/// Faces over the single-unit arena:
///
/// - Library face (the frontier's root push, visitor :51-59): a
///   `@deprecated` `library` directive suppresses every use in the
///   unit — the whole file goes silent.
/// - Same-file reference face (approximation, documented): a
///   reference-context identifier whose lexeme matches a same-file
///   deprecated declaration reports at itself, unless its byte start
///   sits inside any deprecated frontier declaration's span (the
///   ancestor suppression). Pushed shapes mirror the visitor's
///   overrides: class-likes, mixin applications, typedefs, top-level
///   fields/methods, members, constructors/factories, fields, formal
///   parameters (regular and function-typed); locals, closures and
///   enum constants are not pushed upstream and not here.
///   Under-report: cross-file same-package uses through other names.
/// - Directive face (upstream `importDirective`/`exportDirective` +
///   `combinatorName`, detector :769-775/:614-624): an import/export
///   of a library whose `library` directive is deprecated reports at
///   the whole directive with the target's package URI as the name; a
///   `show` combinator name matching a deprecated top-level
///   class-family declaration of the directly imported file reports
///   at the name (`hide` is forgiving and stays silent). The face
///   needs the workspace facts to resolve the URI and compose the
///   `package:` display name — without them it stays silent; the
///   target file is re-scanned at the token level, so only the
///   directly imported file's own declarations answer (re-export
///   chains under-report).
///
/// Messages: `diagnostic.g.dart`
/// `deprecatedMemberUseFromSamePackageWithMessage` and
/// `…WithoutMessage`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "deprecated_member_use_from_same_package",
    &[NodeKind::CompilationUnit],
    check,
);

const WITH_MESSAGE: &str = "'{0}' is deprecated and shouldn't be used. {1}";
const WITHOUT_MESSAGE: &str = "'{0}' is deprecated and shouldn't be used.";
const CORRECTION: &str = "Try replacing the use of the deprecated member with the replacement, \
         if a replacement is specified.";

/// One same-file deprecated declaration: the name lexeme and the
/// normalized message (`None` = the bare `@deprecated` shape).
struct DeprecatedDecl<'a> {
    name: &'a str,
    message: Option<String>,
}

fn check(c: &mut Check<'_>) {
    // The frontier's root push (visitor `visitCompilationUnit`): a
    // deprecated `library` directive silences the whole unit.
    if unit_library_deprecated(c) {
        return;
    }
    // The directive faces — import/export of a deprecated library and
    // the `show` combinator names (workspace-gated: the URI resolution
    // and the `package:` display name are package facts).
    directive_faces(c);
    let decls = collect_deprecated(c, c.node());
    if decls.is_empty() {
        return;
    }
    // One pass over the unit's rows: every deprecated frontier
    // declaration contributes its span (the ancestor suppression), and
    // every reference-context identifier of a deprecated lexeme is a
    // candidate hit. The upstream push set (`_withDeclaration` /
    // `_withFormalParameter` overrides) is `is_frontier_declaration`;
    // a use inside any deprecated declaration's span never reports.
    let len = narrow_u32(c.unit.ast.len());
    let mut suppressed: Vec<core::ops::Range<usize>> = Vec::new();
    let mut hits: Vec<(u32, Option<String>)> = Vec::new();
    for row in 0..len {
        if c.kind(row) == NodeKind::Identifier {
            let Some(context) = c.field(row, PayloadField::Context) else {
                continue;
            };
            if !is_reference_context(context) {
                continue;
            }
            let name = c.lexeme(c.head_token(row));
            let Some(hit) = decls.iter().find(|decl| decl.name == name) else {
                continue;
            };
            hits.push((row, hit.message.clone()));
        } else if is_frontier_declaration(c.kind(row)) && frontier_deprecated(c, row) {
            suppressed.push(frontier_span(c, row));
        }
    }
    for (row, message) in hits {
        let byte = c.byte_span(row);
        if suppressed
            .iter()
            .any(|span| span.start <= byte.start && byte.start < span.end)
        {
            continue;
        }
        let name = c.lexeme(c.head_token(row));
        let range = c.utf16_span(row);
        match message {
            Some(message) => {
                c.report_texts(range, WITH_MESSAGE, CORRECTION, &[name, &message]);
            }
            None => c.report_texts(range, WITHOUT_MESSAGE, CORRECTION, &[name]),
        }
    }
}

/// The declaration shapes the upstream frontier visitor pushes while
/// their contents are visited (`element_usage_frontier_visitor.dart`:
/// the `_withDeclaration` class/enum/extension/mixin/typedef/function
/// overrides, the constructor/method overrides, the field and top-level
/// variable pushes, and the formal-parameter overrides). Enum constants,
/// locals and closures are not pushed upstream and not here.
fn is_frontier_declaration(kind: NodeKind) -> bool {
    matches!(
        kind,
        NodeKind::ClassDeclaration
            | NodeKind::MixinDeclaration
            | NodeKind::ExtensionDeclaration
            | NodeKind::ExtensionTypeDeclaration
            | NodeKind::EnumDeclaration
            | NodeKind::NamedMixinApplication
            | NodeKind::Typedef
            | NodeKind::TopLevelMethod
            | NodeKind::TopLevelFields
            | NodeKind::Method
            | NodeKind::Constructor
            | NodeKind::Factory
            | NodeKind::Fields
            | NodeKind::FormalParameter
            | NodeKind::FunctionTypedFormalParameter
    )
}

/// Whether `row` sits under a deprecated frontier annotation: the
/// metadata star is the row's left sibling (the uniform
/// `parseMetadataStar` emission order — the star parses before the row
/// opens), and class-likes behind a `TopLevelDeclaration` wrapper carry
/// the star beside the wrapper.
fn frontier_deprecated(c: &Check<'_>, row: u32) -> bool {
    if row_is_deprecated(c, row) {
        return true;
    }
    match c.unit.ast.parent(row) {
        Some(wrapper) if c.kind(wrapper) == NodeKind::TopLevelDeclaration => {
            row_is_deprecated(c, wrapper)
        }
        _ => false,
    }
}

/// The suppression span of a frontier declaration: the wrapper's span
/// for the wrapper-nested top-level rows (it covers the hoisted name),
/// the row's own span otherwise — extended to the row's subtree extent,
/// because some tilings seal the row short of its children (the
/// function-typed parameter's outer row seals at `void p` while the
/// nested parameter list rides beside it).
fn frontier_span(c: &Check<'_>, row: u32) -> core::ops::Range<usize> {
    let (base, base_row) = match c.unit.ast.parent(row) {
        Some(wrapper) if c.kind(wrapper) == NodeKind::TopLevelDeclaration => {
            (c.byte_span(wrapper), wrapper)
        }
        _ => (c.byte_span(row), row),
    };
    let window = c.unit.ast.children(base_row);
    let end = (window.start..window.end)
        .map(|kid| c.byte_span(kid).end)
        .max()
        .unwrap_or(base.end)
        .max(base.end);
    base.start..end
}

/// Whether any `library` directive of the unit carries `@deprecated` —
/// the deprecated-library face that silences the whole unit (the
/// frontier's root push). Position-independent: a raw row walk over the
/// `LibraryName` rows. The star read is recovery-tolerant — a mid-file
/// `library` directive (the ordering diagnostic) emits a
/// `RecoverableError` row between the metadata star and the wrapper, so
/// the scan skips recovery rows before requiring the star; nothing but
/// recovery markers can interleave there (the star parses immediately
/// before the directive opens).
fn unit_library_deprecated(c: &Check<'_>) -> bool {
    for row in 0..narrow_u32(c.unit.ast.len()) {
        if c.kind(row) != NodeKind::LibraryName {
            continue;
        }
        if row_is_deprecated(c, row) {
            return true;
        }
        let Some(wrapper) = c.unit.ast.parent(row) else {
            continue;
        };
        if c.kind(wrapper) != NodeKind::TopLevelDeclaration {
            continue;
        }
        let Some(parent) = c.unit.ast.parent(wrapper) else {
            continue;
        };
        let siblings = c.collect_direct(parent);
        let Some(pos) = siblings.iter().position(|&s| s == wrapper) else {
            continue;
        };
        for prev in siblings[..pos].iter().rev().copied() {
            match c.kind(prev) {
                NodeKind::RecoverableError => continue,
                NodeKind::MetadataStar => {
                    if c.collect_direct(prev)
                        .into_iter()
                        .any(|meta| is_deprecated_metadata(c, meta))
                    {
                        return true;
                    }
                    break;
                }
                _ => break,
            }
        }
    }
    false
}

/// The directive faces over the unit's `Import`/`Export` rows. Silent
/// without the workspace facts (no URI resolution, no package URI).
fn directive_faces(c: &mut Check<'_>) {
    let Some(workspace) = c.unit.workspace else {
        return;
    };
    let Some(path) = c.unit.path else {
        return;
    };
    for row in 0..narrow_u32(c.unit.ast.len()) {
        let kind = c.kind(row);
        if kind != NodeKind::Import && kind != NodeKind::Export {
            continue;
        }
        let kids = c.collect_direct(row);
        let Some(uri_row) = kids
            .iter()
            .copied()
            .find(|&kid| c.kind(kid) == NodeKind::StringLiteral)
        else {
            continue;
        };
        let Some(uri) = string_literal_value(c, c.head_token(uri_row)) else {
            continue;
        };
        let Some(target) = resolve_same_package(workspace, path, &uri) else {
            continue;
        };
        let Ok(source) = std::fs::read_to_string(&target) else {
            continue;
        };
        let face = scan_target_deprecations(&source, &target);
        // The library face: the whole directive reports with the
        // target's package URI as the display name (upstream
        // `displayName = element.uri.toString()`).
        if face.library.is_some() {
            if let Some(display) = package_uri(workspace, &target) {
                let range = c.utf16_span(row);
                match face.library.unwrap() {
                    Some(message) => {
                        c.report_texts(range, WITH_MESSAGE, CORRECTION, &[&display, &message]);
                    }
                    None => c.report_texts(range, WITHOUT_MESSAGE, CORRECTION, &[&display]),
                }
            }
        }
        // The `show` combinator face: a shown name matching a deprecated
        // top-level class-family declaration of the target reports at
        // the name (`hide` is forgiving — no `Hide` walk). The rows
        // materialize first: the report borrows `c` mutably.
        let shows: Vec<u32> = kids
            .iter()
            .copied()
            .filter(|&kid| c.kind(kid) == NodeKind::Show)
            .collect();
        for show in shows {
            let names: Vec<u32> = c
                .collect_direct(show)
                .into_iter()
                .filter(|&kid| c.kind(kid) == NodeKind::Identifier)
                .collect();
            for name in names {
                let lexeme = c.lexeme(c.head_token(name));
                let Some((_, message)) = face
                    .declared
                    .iter()
                    .find(|(declared, _)| declared.as_str() == lexeme)
                else {
                    continue;
                };
                let range = c.utf16_span(name);
                match message {
                    Some(message) => {
                        c.report_texts(range, WITH_MESSAGE, CORRECTION, &[lexeme, message]);
                    }
                    None => c.report_texts(range, WITHOUT_MESSAGE, CORRECTION, &[lexeme]),
                }
            }
        }
    }
}

/// One `Import`/`Export` row's directive-face verdicts, read off the
/// target file: the deprecated `library` directive (`Some(message)` —
/// `None` inside for the bare `@deprecated` shape) and the deprecated
/// top-level class-family names for the combinator face.
#[derive(Default)]
struct TargetDeprecations {
    library: Option<Option<String>>,
    declared: Vec<(String, Option<String>)>,
}

/// The URI literal's value: the lexeme with one level of quotes
/// stripped (interpolation segments stay unreported).
fn unquote(raw: &str) -> Option<&str> {
    let bytes = raw.as_bytes();
    if bytes.len() >= 2
        && (bytes[0] == b'\'' || bytes[0] == b'"')
        && bytes[0] == bytes[bytes.len() - 1]
    {
        Some(&raw[1..raw.len() - 1])
    } else {
        None
    }
}

/// The URI literal's value off the unit's own token table.
fn string_literal_value<'a>(c: &'a Check<'_>, token: u32) -> Option<&'a str> {
    unquote(c.lexeme(token))
}

/// The file a same-package URI resolves to: `package:<own>/…` through
/// the package's URI directory, anything else relative to the importing
/// file. Cross-package and SDK URIs answer `None` (those uses are the
/// analyzer's `DEPRECATED_MEMBER_USE`, not this rule's); a relative
/// target that escapes the analyzed package too (the `isInSamePackage`
/// gate) answers `None`.
fn resolve_same_package(
    workspace: &crate::WorkspaceFacts,
    path: &Path,
    uri: &str,
) -> Option<PathBuf> {
    if let Some(rest) = uri.strip_prefix("package:") {
        let own = format!("{}/", workspace.own_package);
        let relative = rest.strip_prefix(&own)?;
        return Some(workspace.lib_dir.join(relative));
    }
    if uri.starts_with("dart:") {
        return None;
    }
    let base = path.parent()?;
    let target = base.join(uri);
    target.starts_with(&workspace.root).then_some(target)
}

/// The target's `package:` display URI — `package:<own>/<path under the
/// package URI directory>`; a target outside it has no package URI and
/// stays unreported (the official face's `element.uri` has no
/// `package:` form for it either).
fn package_uri(workspace: &crate::WorkspaceFacts, target: &Path) -> Option<String> {
    let relative = target.strip_prefix(&workspace.lib_dir).ok()?;
    let mut display = format!("package:{}", workspace.own_package);
    for component in relative.components() {
        display.push('/');
        display.push_str(&component.as_os_str().to_string_lossy());
    }
    Some(display)
}

/// The token-level deprecation scan of a target file: the deprecation
/// annotation directly preceding a `library` directive, and the
/// deprecated top-level class-family declarations (the shapes the
/// directive faces read). Token-level so comments and strings never
/// answer; the `library` face wins over the per-declaration list.
fn scan_target_deprecations(source: &str, path: &Path) -> TargetDeprecations {
    let mut face = TargetDeprecations::default();
    let mut files = dartq_source::FileTable::default();
    let tokens = dartq_scanner::scan(
        source,
        dartq_scanner::ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        files.intern(path),
    )
    .tokens;
    let lexeme = |index: usize| tokens.lexeme(source, index);
    let kind = |index: usize| tokens.get(index).map(|token| token.kind);
    let mut index = 0;
    while index < tokens.len() {
        if lexeme(index) != "@" {
            index += 1;
            continue;
        }
        // The annotation name must hug the `@`.
        let Some(name_token) = (tokens.get(index + 1))
            .filter(|token| token.kind == dartq_scanner::TokenKind::Identifier)
            .map(|_| index + 1)
        else {
            index += 1;
            continue;
        };
        let name = lexeme(name_token);
        if name != "deprecated" && name != "Deprecated" {
            index += 1;
            continue;
        }
        // `@Deprecated('…')`: the first string argument is the message,
        // run through the official normalization.
        let mut cursor = name_token + 1;
        let mut message = None;
        if kind(cursor) == Some(dartq_scanner::TokenKind::OpenParen) {
            let mut depth = 0usize;
            let mut close = cursor;
            while close < tokens.len() {
                match kind(close) {
                    Some(dartq_scanner::TokenKind::OpenParen) => depth += 1,
                    Some(dartq_scanner::TokenKind::CloseParen) => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    Some(dartq_scanner::TokenKind::Eof) => break,
                    Some(dartq_scanner::TokenKind::String) if depth == 1 && message.is_none() => {
                        let token = tokens.get(close);
                        let raw = token.map(|token| {
                            &source[token.byte_offset as usize
                                ..(token.byte_offset + token.byte_len) as usize]
                        });
                        message = normalize_deprecation_message(raw.and_then(unquote));
                    }
                    _ => {}
                }
                close += 1;
            }
            cursor = close + 1;
        }
        // The annotated head decides the face: `library` (the directive
        // face) or the class-family keyword run (the combinator face).
        // Every read clamps to the table (the Eof terminator's empty
        // lexeme answers every keyword probe) — a malformed target
        // degrades to silence, never out of bounds.
        let last = tokens.len() - 1;
        let cursor = cursor.min(last);
        let head = lexeme(cursor);
        if head == "library" {
            face.library = Some(message);
        } else if matches!(
            head,
            "class" | "mixin" | "enum" | "extension" | "type" | "typedef"
        ) {
            let mut head = cursor;
            while head < last
                && matches!(
                    lexeme(head),
                    "class"
                        | "mixin"
                        | "enum"
                        | "extension"
                        | "type"
                        | "typedef"
                        | "sealed"
                        | "base"
                        | "abstract"
                        | "final"
                        | "augment"
                )
            {
                head += 1;
            }
            if kind(head) == Some(dartq_scanner::TokenKind::Identifier) {
                face.declared.push((lexeme(head).to_owned(), message));
            }
        }
        index = cursor.max(index + 1);
    }
    face
}

/// `normalizeDeprecationMessage`
/// (deprecated_member_use_verifier.dart:18-29): trimmed; empty and `.`
/// normalize to `None`; a trailing `.`, `?` or `!` stays as written,
/// anything else gains a `.`.
fn normalize_deprecation_message(value: Option<&str>) -> Option<String> {
    let trimmed = value?.trim();
    if trimmed.is_empty() || trimmed == "." {
        return None;
    }
    if trimmed.ends_with(['.', '?', '!']) {
        Some(trimmed.to_owned())
    } else {
        Some(format!("{trimmed}."))
    }
}

/// Whether `row` carries `@deprecated` metadata — the metadata star is
/// the row's left sibling in its parent's children (the uniform
/// `parseMetadataStar` emission order for declarations, members, formal
/// parameters and directives).
fn row_is_deprecated(c: &Check<'_>, row: u32) -> bool {
    let Some(parent) = c.unit.ast.parent(row) else {
        return false;
    };
    let siblings = c.collect_direct(parent);
    let Some(pos) = siblings.iter().position(|&s| s == row) else {
        return false;
    };
    let Some(&star) = pos.checked_sub(1).and_then(|i| siblings.get(i)) else {
        return false;
    };
    if c.kind(star) != NodeKind::MetadataStar {
        return false;
    }
    c.collect_direct(star)
        .into_iter()
        .any(|meta| is_deprecated_metadata(c, meta))
}

/// The unit's deprecated declarations, in declaration order: the
/// top-level rows (the wrapper carries the metadata star as its left
/// sibling — hand-built trees without the wrapper pass the row itself)
/// and the members of every class-like container.
fn collect_deprecated<'a>(c: &Check<'a>, root: u32) -> Vec<DeprecatedDecl<'a>> {
    let mut out = Vec::new();
    for top in c.collect_direct(root) {
        // A `TopLevelDeclaration` wrapper carries the metadata star as
        // its own left sibling. The name `Identifier` lives inside the
        // declaration row for methods/fields/typedefs, but class-likes
        // hoist it onto the wrapper's direct children — `name_row` is
        // where the name was stamped, `row` the declaration itself.
        let row = if c.kind(top) == NodeKind::TopLevelDeclaration {
            match c.collect_direct(top).into_iter().find(|&kid| {
                matches!(
                    c.kind(kid),
                    NodeKind::ClassDeclaration
                        | NodeKind::MixinDeclaration
                        | NodeKind::ExtensionDeclaration
                        | NodeKind::ExtensionTypeDeclaration
                        | NodeKind::EnumDeclaration
                        | NodeKind::NamedMixinApplication
                        | NodeKind::Typedef
                        | NodeKind::TopLevelMethod
                        | NodeKind::TopLevelFields
                )
            }) {
                Some(decl) => decl,
                None => continue,
            }
        } else {
            top
        };
        // Only the class-likes hoist their name onto the wrapper; every
        // other declaration keeps its name `Identifier` inside the row.
        let name_row = match c.kind(row) {
            NodeKind::ClassDeclaration
            | NodeKind::MixinDeclaration
            | NodeKind::ExtensionDeclaration
            | NodeKind::ExtensionTypeDeclaration
            | NodeKind::EnumDeclaration => {
                if c.kind(top) == NodeKind::TopLevelDeclaration {
                    top
                } else {
                    row
                }
            }
            _ => row,
        };
        match c.kind(row) {
            NodeKind::ClassDeclaration
            | NodeKind::MixinDeclaration
            | NodeKind::ExtensionDeclaration
            | NodeKind::ExtensionTypeDeclaration
            | NodeKind::EnumDeclaration => {
                let (_, members) = container_members(c, row);
                collect_row(c, name_row, row, &mut out);
                collect_members(c, &members, &mut out);
            }
            NodeKind::NamedMixinApplication
            | NodeKind::Typedef
            | NodeKind::TopLevelMethod
            | NodeKind::TopLevelFields => collect_row(c, name_row, row, &mut out),
            _ => {}
        }
    }
    out
}

/// One top-level row's own metadata star. `name_row` is the row the
/// declaration's name `Identifier` sits on — the wrapper for real parses
/// (the name hoists), the row itself for hand-built tables.
fn collect_row<'a>(c: &Check<'a>, name_row: u32, row: u32, out: &mut Vec<DeprecatedDecl<'a>>) {
    let metas = top_level_annotations(c, row);
    push_decl(c, name_row, row, &metas, out);
}

/// The container member rows: each member's `MetadataStar` is its left
/// sibling (the `member_annotations` idiom, walked in one pass).
fn collect_members<'a>(c: &Check<'a>, members: &[u32], out: &mut Vec<DeprecatedDecl<'a>>) {
    for (index, &member) in members.iter().enumerate() {
        if !matches!(
            c.kind(member),
            NodeKind::Method
                | NodeKind::Constructor
                | NodeKind::Factory
                | NodeKind::Fields
                | NodeKind::EnumElement
        ) {
            continue;
        }
        let Some(&star) = index.checked_sub(1).and_then(|i| members.get(i)) else {
            continue;
        };
        if c.kind(star) != NodeKind::MetadataStar {
            continue;
        }
        let metas: Vec<u32> = c
            .collect_direct(star)
            .into_iter()
            .filter(|&meta| c.kind(meta) == NodeKind::Metadata)
            .collect();
        push_decl(c, member, member, &metas, out);
    }
}

/// The declaration's name faces and one `DeprecatedDecl` per deprecated
/// metadata row. `name_row` is where the name `Identifier` was stamped —
/// the `TopLevelDeclaration` wrapper on real parses (class-like names
/// hoist to the wrapper, the declaration row holds the body), the row
/// itself for members and hand-built tables. `row` is the declaration row
/// the metadata star precedes and whose span is the frontier. Fields name
/// through their `InitializedIdentifier` rows (one entry per declared
/// variable, the row span shared); methods and constructors name through
/// their context-stamped `Identifier` children — a named constructor keys
/// on its type name (the `.name` side is an invocation selector, reported
/// at the type name).
fn push_decl<'a>(
    c: &Check<'a>,
    name_row: u32,
    row: u32,
    metas: &[u32],
    out: &mut Vec<DeprecatedDecl<'a>>,
) {
    if metas.is_empty() {
        return;
    }
    let deprecated: Vec<u32> = metas
        .iter()
        .copied()
        .filter(|&meta| is_deprecated_metadata(c, meta))
        .collect();
    if deprecated.is_empty() {
        return;
    }
    let names: Vec<&str> = match c.kind(row) {
        NodeKind::Fields | NodeKind::TopLevelFields => {
            let mut names = Vec::new();
            for decl in c.collect_direct(row) {
                if c.kind(decl) != NodeKind::VariablesDeclaration {
                    continue;
                }
                for var in c.collect_direct(decl) {
                    if let Some(token) = initialized_identifier_name(c, var) {
                        names.push(c.lexeme(token));
                    }
                }
            }
            names
        }
        NodeKind::Method => c
            .collect_direct(name_row)
            .into_iter()
            .filter(|&kid| {
                c.kind(kid) == NodeKind::Identifier
                    && c.field(kid, PayloadField::Context)
                        == Some(dartq_parser::IdentifierContext::MethodDeclaration as u32)
            })
            .map(|kid| c.lexeme(c.head_token(kid)))
            .collect(),
        NodeKind::TopLevelMethod => c
            .collect_direct(name_row)
            .into_iter()
            .filter(|&kid| {
                c.kind(kid) == NodeKind::Identifier
                    && c.field(kid, PayloadField::Context)
                        == Some(dartq_parser::IdentifierContext::TopLevelFunctionDeclaration as u32)
            })
            .map(|kid| c.lexeme(c.head_token(kid)))
            .collect(),
        // Class-likes, typedefs, enum constants, constructors (the type
        // name): the first `Identifier` child of the name row — except
        // the mixin application, whose alias name rides the wrapper as
        // the declaration-context identifier (its row's own identifier
        // children start at the `=` supertype).
        NodeKind::NamedMixinApplication => {
            let name = c
                .unit
                .ast
                .parent(row)
                .map(|parent| c.collect_direct(parent))
                .and_then(|siblings| {
                    let pos = siblings.iter().position(|&s| s == row)?;
                    siblings[..pos].iter().rev().copied().find(|&kid| {
                        c.kind(kid) == NodeKind::Identifier
                            && c.field(kid, PayloadField::Context)
                                == Some(
                                    dartq_parser::IdentifierContext::
                                        ClassOrMixinOrExtensionDeclaration as u32,
                                )
                    })
                });
            match name {
                Some(kid) => vec![c.lexeme(c.head_token(kid))],
                None => return,
            }
        }
        _ => match c
            .collect_direct(name_row)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::Identifier)
        {
            Some(kid) => vec![c.lexeme(c.head_token(kid))],
            None => return,
        },
    };
    for meta in deprecated {
        let message = deprecation_message(c, meta);
        for &name in &names {
            out.push(DeprecatedDecl {
                name,
                message: message.clone(),
            });
        }
    }
}

/// The `@Deprecated('…')` tag: the first argument's string value run
/// through `normalizeDeprecationMessage`
/// (deprecated_member_use_verifier.dart:18-29) — trimmed; empty and `.`
/// normalize to `None` (the without-message variant); a trailing `.`,
/// `?` or `!` stays as written, anything else gains a `.`. `None`
/// without the outer face for a bare `@deprecated` (no arguments) or an
/// unreadable tag.
fn deprecation_message(c: &Check<'_>, meta: u32) -> Option<String> {
    let argument = c
        .collect_direct(meta)
        .into_iter()
        .find(|&piece| c.kind(piece) == NodeKind::Arguments)
        .and_then(|args| c.collect_direct(args).into_iter().next())
        .filter(|&lit| c.kind(lit) == NodeKind::StringLiteral);
    let value = crate::rules::directives::string_value(c, argument?)?;
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed == "." {
        return None;
    }
    if trimmed.ends_with(['.', '?', '!']) {
        Some(trimmed.to_owned())
    } else {
        Some(format!("{trimmed}."))
    }
}

/// The reference-context faces of the identifier-context payload: bare
/// expressions and member selectors, type references (plain, prefixed
/// and continuations), constructor references. Declaration names,
/// parameters, metadata, labels, named-argument labels and symbols
/// never report.
fn is_reference_context(context: u32) -> bool {
    use dartq_parser::IdentifierContext;
    context == IdentifierContext::Expression as u32
        || context == IdentifierContext::ExpressionContinuation as u32
        || context == IdentifierContext::TypeReference as u32
        || context == IdentifierContext::PrefixedTypeReference as u32
        || context == IdentifierContext::TypeReferenceContinuation as u32
        || context == IdentifierContext::ConstructorReference as u32
        || context == IdentifierContext::ConstructorReferenceContinuation as u32
        || context == IdentifierContext::ConstructorReferenceContinuationAfterTypeArguments as u32
}
