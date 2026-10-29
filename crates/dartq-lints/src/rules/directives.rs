//! The directive family (the #300 parser milestone face):
//! `avoid_relative_lib_imports`, `library_annotations`, `library_prefixes`,
//! `no_leading_underscores_for_library_prefixes`, `unnecessary_library_name`,
//! `use_string_in_part_of_directives`, `library_names`,
//! `package_prefixed_library_names`, `directives_ordering`.
//!
//! The payload contracts are the bridge's directive layout: `Import.as/deferred`
//! (`ImportPrefixHandle` back-fill), `LibraryName.has_name`,
//! `PartOf.of/has_name`. Annotations attach as `Metadata` nodes under a
//! `MetadataStar` container that precedes the annotated node (the parser
//! opens declaration nodes after `parseMetadataStar`), so `library_annotations`
//! reads the annotated node as the star's next sibling.

use dartq_ast::{NodeKind, PayloadField};

use crate::engine::Check;
use crate::registry::RuleSpec;

/// The domain's incoming registrations: a new standalone rule appends its
/// `SPEC` reference here (name order) and joins the registry through the
/// aggregated tail — adding a rule touches only this file. Batch close
/// promotes the block into the registry's frozen skeleton together with
/// the pin-test wave list (`tests/registry_pin.rs`).
pub(crate) static INCOMING_RULES: &[&RuleSpec] = &[];

/// The node behind a directive prelude wrapper —
/// `begin_uncategorized_top_level_declaration` opens `TopLevelDeclaration`
/// around every directive and the five class-family preludes,
/// so a unit child that is a wrapper is read through its first
/// child.
fn behind_prelude(c: &Check<'_>, node: u32) -> u32 {
    if c.kind(node) != NodeKind::TopLevelDeclaration {
        return node;
    }
    c.collect_direct(node).first().copied().unwrap_or(node)
}

/// The import prefix identifier — the only direct [`NodeKind::Identifier`]
/// child of an [`NodeKind::Import`] (combinator names live inside Show/Hide
/// nodes, conditional-URI dotted names inside `ConditionalUri`).
fn import_prefix(c: &Check<'_>, node: u32) -> Option<u32> {
    c.collect_direct(node)
        .into_iter()
        .find(|&k| c.kind(k) == NodeKind::Identifier)
}

/// Whether the written prefixed name `prefix.name` resolves to dart:core's
/// `name` through a same-file import — the import-qualified face of the
/// element-model library pin (`libraryName: 'dart.core'`, an
/// `ElementAnnotation`'s resolution): one of the unit's imports of the
/// written `dart:core` URI under this prefix, not deferred (a deferred
/// import's constants never load), whose combinators admit the name (a
/// `show` list must contain it, a `hide` list must not).
pub(crate) fn prefixed_dart_core_name(c: &Check<'_>, prefix: &str, name: &str) -> bool {
    let Some(&unit) = c
        .path
        .iter()
        .find(|&&n| c.kind(n) == NodeKind::CompilationUnit)
    else {
        return false;
    };
    for kid in c.collect_direct(unit) {
        let row = behind_prelude(c, kid);
        if c.kind(row) != NodeKind::Import {
            continue;
        }
        let Some(prefix_row) = import_prefix(c, row) else {
            continue;
        };
        if c.lexeme(c.head_token(prefix_row)) != prefix {
            continue;
        }
        // The `DeferredKeyword` word carries the `deferred` keyword token
        // (`TOKEN_NONE` when the import is not deferred) — a deferred
        // import's constants never load.
        if c.field(row, PayloadField::DeferredKeyword)
            .unwrap_or(crate::rules::NONE_TOKEN)
            != crate::rules::NONE_TOKEN
        {
            continue;
        }
        let Some((_, uri)) = written_uri(c, row) else {
            continue;
        };
        if uri != "dart:core" {
            continue;
        }
        let mut shown: Option<Vec<&str>> = None;
        let mut hidden = false;
        for combinator in c.collect_direct(row) {
            match c.kind(combinator) {
                NodeKind::Show if shown.is_none() => {
                    shown = Some(combinator_names(c, combinator));
                }
                NodeKind::Hide if !hidden => {
                    hidden = combinator_names(c, combinator).contains(&name);
                }
                _ => {}
            }
        }
        if hidden {
            continue;
        }
        if shown.is_some_and(|names| !names.contains(&name)) {
            continue;
        }
        return true;
    }
    false
}

/// The annotation name face of a [`NodeKind::Metadata`] row: `(prefix,
/// name)` — the bridge lays a prefixed `foo.bar` out as two
/// [`NodeKind::Identifier`] children with the period in the payload, a
/// bare `bar` as one.
fn metadata_name_face(c: &Check<'_>, meta: u32) -> Option<(Option<u32>, u32)> {
    let names: Vec<u32> = c
        .collect_direct(meta)
        .into_iter()
        .filter(|&k| c.kind(k) == NodeKind::Identifier)
        .collect();
    match names.as_slice() {
        [name] => Some((None, *name)),
        [prefix, name] if c.field(meta, PayloadField::PeriodBeforeName).unwrap_or(0) != 0 => {
            Some((Some(*prefix), *name))
        }
        _ => None,
    }
}

/// The names of a `show`/`hide` combinator (its `Identifier` children).
fn combinator_names<'a>(c: &'a Check<'_>, combinator: u32) -> Vec<&'a str> {
    c.collect_direct(combinator)
        .into_iter()
        .filter(|&kid| c.kind(kid) == NodeKind::Identifier)
        .map(|kid| c.lexeme(c.head_token(kid)))
        .collect()
}

/// The URI of an import: the leading run of `StringLiteral` direct children
/// (the adjacent-string concatenation face — `AdjacentStringLiteralsHandle`
/// collapses away, consecutive segments in the parent).
pub(crate) fn leading_string_run(c: &Check<'_>, node: u32) -> Vec<u32> {
    c.collect_direct(node)
        .into_iter()
        .take_while(|&k| c.kind(k) == NodeKind::StringLiteral)
        .collect()
}

/// The written URI of an import/export directive: the cooked
/// adjacent-string run (`stringValue`, None on interpolation) with its
/// UTF-16 span — the official `node.uri` report anchor. `None` when the
/// directive has no readable string URI.
pub(crate) fn written_uri(c: &Check<'_>, node: u32) -> Option<(core::ops::Range<u32>, String)> {
    let run = leading_string_run(c, node);
    let first = *run.first()?;
    let last = *run.last()?;
    let value = run
        .iter()
        .map(|&n| string_value(c, n))
        .collect::<Option<String>>()?;
    let start = c.utf16_span(first).start;
    let end = c.utf16_span(last).end;
    Some((start..end, value))
}

/// The configured URI of a conditional-import clause — the last
/// [`NodeKind::StringLiteral`] direct child of the
/// [`NodeKind::ConditionalUri`] (the bridge keeps the clause's string
/// events as direct children; an `==` comparison value, when present,
/// precedes the candidate). The cooked value with its UTF-16 span — the
/// official `configuration.uri` report anchor. `None` on interpolation.
pub(crate) fn candidate_uri(c: &Check<'_>, node: u32) -> Option<(core::ops::Range<u32>, String)> {
    let literal = c
        .collect_direct(node)
        .into_iter()
        .rfind(|&kid| c.kind(kid) == NodeKind::StringLiteral)?;
    Some((c.utf16_span(literal), string_value(c, literal)?))
}

/// The value of one `StringLiteral` node — the official `stringValue`
/// (`pkg/linter/.../ast.dart`): quote-stripped, escapes cooked, `None` on
/// interpolation or a malformed literal. The escape walk is the Dart
/// short-escape/`\xHH`/`\uHHHH`/`\u{H…}`/passthrough set (the engine's
/// cooked-string processor, dartq-engine/src/l1.rs `string_literal_value` —
/// sibling crates, so the spec-mandated walk is mirrored here).
pub(crate) fn string_value(c: &Check<'_>, node: u32) -> Option<String> {
    let text = &c.unit.source[c.byte_span(node)];
    let is_raw = text.starts_with('r');
    let body = if is_raw { &text[1..] } else { text };
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
    cook_string_body(body)
}

/// The escape-cooking walk behind [`string_value`], shared with the
/// interpolation-segment readers (the per-segment contents of an
/// interpolated literal carry the same escape grammar minus the
/// interpolation markers, which never occur inside a segment).
pub(crate) fn cook_string_body(body: &str) -> Option<String> {
    let chars: Vec<char> = body.chars().collect();
    let mut out = String::with_capacity(body.len());
    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];
        i += 1;
        if ch != '\\' {
            // Unescaped `$` + `{`/identifier-start begins interpolation; the
            // official stringValue is null there.
            if ch == '$'
                && chars
                    .get(i)
                    .is_some_and(|&n| n == '{' || n == '_' || n.is_alphanumeric())
            {
                return None;
            }
            out.push(ch);
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
                let digits: String = chars.get(i + 1..i + 3)?.iter().collect();
                i += 2;
                out.push(char::from_u32(u32::from_str_radix(&digits, 16).ok()?)?);
            }
            'u' => {
                i += 1;
                let digits = if chars.get(i) == Some(&'{') {
                    let start = i + 1;
                    let end = chars[start..].iter().position(|&c| c == '}')? + start;
                    i = end;
                    chars[start..end].iter().collect::<String>()
                } else {
                    let digits = chars.get(i..i + 4)?.iter().collect::<String>();
                    i += 3;
                    digits
                };
                out.push(char::from_u32(u32::from_str_radix(&digits, 16).ok()?)?);
            }
            // Backslash-newline elides (multiline bodies).
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

pub(crate) mod avoid_relative_lib_imports {
    use super::{Check, NodeKind, RuleSpec};

    /// Official: `pkg/linter/lib/src/rules/avoid_relative_lib_imports.dart:23-49`
    /// — a relative import URI whose path contains `/lib/` reports at the
    /// URI. `isRelativeLibImport` (:24-35): `Uri.tryParse(uri.stringValue)`,
    /// scheme empty, `uri.path.contains('/lib/')`. Message:
    /// `diagnostic.g.dart` `avoidRelativeLibImports`.
    pub(crate) static SPEC: RuleSpec =
        crate::spec!("avoid_relative_lib_imports", &[NodeKind::Import], check,);

    /// The scheme prefix length (`letter (letter | digit | + - .)* ':'`, the
    /// `Uri` scheme grammar) — non-empty schemes disqualify (:28-32).
    pub(crate) fn scheme_prefix_len(value: &str) -> Option<usize> {
        let b = value.as_bytes();
        if !b.first().is_some_and(|&c| c.is_ascii_alphabetic()) {
            return None;
        }
        let mut i = 1;
        while i < b.len() && (b[i].is_ascii_alphanumeric() || matches!(b[i], b'+' | b'-' | b'.')) {
            i += 1;
        }
        (b.get(i) == Some(&b':')).then_some(i + 1)
    }

    fn check(c: &mut Check<'_>) {
        let run = super::leading_string_run(c, c.node());
        if run.is_empty() {
            return;
        }
        let Some(value) = run
            .iter()
            .map(|&n| super::string_value(c, n))
            .collect::<Option<String>>()
        else {
            return;
        };
        if scheme_prefix_len(&value).is_some() {
            return;
        }
        // `uri.path` — the portion before `?`/`#` (:33).
        let path_end = value.find(['?', '#']).unwrap_or(value.len());
        if !value[..path_end].contains("/lib/") {
            return;
        }
        let range =
            c.utf16_span(run[0]).start..c.utf16_span(*run.last().expect("run is non-empty")).end;
        c.report(range, &[]);
    }
}

pub(crate) mod library_annotations {
    use super::{Check, NodeKind, RuleSpec};

    /// Official: `pkg/linter/lib/src/rules/library_annotations.dart:29-95` —
    /// a library-targeted annotation attached where it does not belong
    /// reports at the annotation. The unit walk (:48-58) stops at a
    /// `part of`, skips the `library` directive itself, and checks every
    /// other directive plus every declaration.
    ///
    /// The official face splits in two. The `targetKinds` branch (:61-66)
    /// reads the annotation declaration's `@Target` metadata through the
    /// element model; the dartq face reads it same-file — a declaration
    /// carrying the annotation's name whose metadata pins exactly
    /// [`TargetKind.library`] reports when the annotated node is the unit's
    /// first directive (`firstDirective == node`, :65). The
    /// `isPragmaLateTrust` branch (:84-95) pins the dart:core `pragma`
    /// constructor; the dartq face reads the written name bare or
    /// import-qualified through a same-file prefix.
    pub(crate) static SPEC: RuleSpec =
        crate::spec!("library_annotations", &[NodeKind::Metadata], check,);

    fn check(c: &mut Check<'_>) {
        // Top-level annotations only: the official visits unit directives
        // and unit declarations (:48-58); a member's metadata is never
        // walked. Chain: MetadataStar -> Metadata under the unit.
        let Some(star) = c.ancestor(1) else {
            return;
        };
        let Some(unit) = c.ancestor(2) else {
            return;
        };
        if c.kind(unit) != NodeKind::CompilationUnit {
            return;
        }
        let kids = c.collect_direct(unit);
        // A part-of unit ends the walk (:51-53). Directives wrap in a
        // TopLevelDeclaration prelude, so read each
        // unit child through the wrapper.
        if kids
            .iter()
            .any(|&k| c.kind(super::behind_prelude(c, k)) == NodeKind::PartOf)
        {
            return;
        }
        // The annotated node follows the metadata star.
        let Some(pos) = kids.iter().position(|&k| k == star) else {
            return;
        };
        let Some(&annotated) = kids.get(pos + 1) else {
            return;
        };
        // The `library` directive is never checked (:54-55).
        if c.kind(super::behind_prelude(c, annotated)) == NodeKind::LibraryName {
            return;
        }
        // The targetKinds branch (:61-66): a same-file annotation
        // declaration pinning exactly `TargetKind.library` reports on the
        // unit's first directive. `firstDirective == node` compares the
        // unit-child rows (the official reads both out of
        // `unit.directives`, whose entries are the same rows the prelude
        // wrapper carries).
        if let Some((_, name)) = super::metadata_name_face(c, c.node()) {
            let first_directive = kids
                .iter()
                .find(|&&k| is_directive(c, super::behind_prelude(c, k)))
                .copied();
            if first_directive == Some(annotated)
                && same_file_targets_library_only(c, c.lexeme(c.head_token(name)))
            {
                let range = c.utf16_span(c.node());
                c.report(range, &[]);
                return;
            }
        }
        if !is_pragma_late_trust(c, c.node()) {
            return;
        }
        let range = c.utf16_span(c.node());
        c.report(range, &[]);
    }

    /// Whether the node is one of the unit directives the official walk
    /// visits (:43-49).
    fn is_directive(c: &Check<'_>, node: u32) -> bool {
        matches!(
            c.kind(node),
            NodeKind::Import
                | NodeKind::Export
                | NodeKind::Part
                | NodeKind::PartOf
                | NodeKind::LibraryName
        )
    }

    /// The targetKinds branch (:61-66), the written face of
    /// `elementAnnotation.targetKinds`: a same-file declaration carrying
    /// `name` whose `@Target` metadata pins exactly `TargetKind.library` —
    /// the kind segments of the `@Target` arguments (each
    /// `TargetKind.<kind>` qualified name), exactly one and `library`
    /// (`targetKinds.length == 1`, :63).
    fn same_file_targets_library_only(c: &Check<'_>, name: &str) -> bool {
        let Some(&unit) = c
            .path
            .iter()
            .find(|&&n| c.kind(n) == NodeKind::CompilationUnit)
        else {
            return false;
        };
        let kids = c.collect_direct(unit);
        for pos in 0..kids.len() {
            // The declaration's name: a top-level declaration wraps in a
            // `TopLevelDeclaration` prelude whose direct members lead with
            // the name `Identifier` (the class/variable header pieces are
            // siblings of the declaration node), so the name reads off the
            // unit child itself — `behind_prelude`'s first member would be
            // that `Identifier`, never a declaration node. A metadata star
            // or a directive wrapper carries no direct `Identifier`.
            let Some(name_row) = c
                .collect_direct(kids[pos])
                .into_iter()
                .find(|&k| c.kind(k) == NodeKind::Identifier)
            else {
                continue;
            };
            if c.lexeme(c.head_token(name_row)) != name || pos == 0 {
                continue;
            }
            // The declaration's metadata star precedes it.
            if c.kind(kids[pos - 1]) != NodeKind::MetadataStar {
                continue;
            }
            for meta in c.collect_direct(kids[pos - 1]) {
                if c.kind(meta) != NodeKind::Metadata {
                    continue;
                }
                let Some((_, segment)) = super::metadata_name_face(c, meta) else {
                    continue;
                };
                if c.lexeme(c.head_token(segment)) != "Target" {
                    continue;
                }
                if target_kinds(c, meta).is_some_and(|kinds| kinds == ["library"]) {
                    return true;
                }
            }
        }
        false
    }

    /// The `@Target` annotation's kind list: the segments of the qualified
    /// names inside its arguments (`TargetKind.library` — each identifier
    /// whose preceding token is a dot), `None` without an argument list.
    fn target_kinds(c: &Check<'_>, meta: u32) -> Option<Vec<String>> {
        let arguments = c
            .collect_direct(meta)
            .into_iter()
            .find(|&k| c.kind(k) == NodeKind::Arguments)?;
        let span = c.byte_span(arguments);
        let mut kinds = Vec::new();
        for row in c.children(arguments) {
            if c.kind(row) != NodeKind::Identifier {
                continue;
            }
            let row_span = c.byte_span(row);
            if row_span.start < span.start || row_span.end > span.end {
                continue;
            }
            let head = c.head_token(row);
            if head == 0 {
                continue;
            }
            if let Some(prev) = c.unit.tokens.get(head as usize - 1) {
                if prev.kind == dartq_scanner::TokenKind::Period {
                    kinds.push(c.lexeme(head).to_owned());
                }
            }
        }
        Some(kinds)
    }

    /// The syntax face of `isPragmaLateTrust` (:84-95): the official pins
    /// the dart:core `pragma` constructor through the element model. The
    /// written face: the annotation carries a `pragma` name — bare (bailing
    /// when a same-file declaration shadows it) or import-qualified through
    /// a same-file prefix ([`super::prefixed_dart_core_name`], the
    /// `libraryName: 'dart.core'` pin). Exactly one positional string
    /// argument `'dart2js:late:trust'`; the named `name:` form is a compile
    /// error upstream (the factory takes one positional) and never fires.
    fn is_pragma_late_trust(c: &Check<'_>, meta: u32) -> bool {
        let Some((prefix, name)) = super::metadata_name_face(c, meta) else {
            return false;
        };
        if c.lexeme(c.head_token(name)) != "pragma" {
            return false;
        }
        match prefix {
            None => {
                if crate::rules::expressions::same_file::declares_name(c, "pragma") {
                    return false;
                }
            }
            Some(prefix) => {
                if !super::prefixed_dart_core_name(c, c.lexeme(c.head_token(prefix)), "pragma") {
                    return false;
                }
            }
        }
        let Some(arguments) = c
            .collect_direct(meta)
            .into_iter()
            .find(|&k| c.kind(k) == NodeKind::Arguments)
        else {
            return false;
        };
        let argv = c.collect_direct(arguments);
        if argv.len() != 1 {
            return false;
        }
        c.kind(argv[0]) == NodeKind::StringLiteral
            && super::string_value(c, argv[0]).as_deref() == Some("dart2js:late:trust")
    }
}

pub(crate) mod library_prefixes {
    use super::{Check, NodeKind, RuleSpec};

    /// Official: `pkg/linter/lib/src/rules/library_prefixes.dart:31-56` — an
    /// import prefix must be `lowercase_with_underscores`:
    /// `isValidLibraryPrefix` (`pkg/linter/lib/src/utils.dart:58`,
    /// `^\$?_*[a-z][_a-z\d]*$`), the `_` wildcard exempt when
    /// `wildcard_variables` is on (:36-39). Message: `diagnostic.g.dart`
    /// `libraryPrefixes`.
    pub(crate) static SPEC: RuleSpec =
        crate::spec!("library_prefixes", &[NodeKind::Import], check,);

    /// `utils.dart:58` — optional `$`, then underscores, then a lowercase
    /// letter, then `[_a-z\d]*`.
    fn is_valid_library_prefix(name: &str) -> bool {
        let b = name.as_bytes();
        let mut i = usize::from(b.first() == Some(&b'$'));
        while i < b.len() && b[i] == b'_' {
            i += 1;
        }
        if i >= b.len() || !b[i].is_ascii_lowercase() {
            return false;
        }
        b[i + 1..]
            .iter()
            .all(|&c| c == b'_' || c.is_ascii_lowercase() || c.is_ascii_digit())
    }

    fn check(c: &mut Check<'_>) {
        let Some(prefix) = super::import_prefix(c, c.node()) else {
            return;
        };
        let name = c.lexeme(c.head_token(prefix));
        if c.unit.features.wildcard_variables && name == "_" {
            return;
        }
        if is_valid_library_prefix(name) {
            return;
        }
        let range = c.utf16_span(prefix);
        c.report(range, &[name]);
    }
}

pub(crate) mod no_leading_underscores_for_library_prefixes {
    use super::{Check, NodeKind, RuleSpec};

    /// Official: `pkg/linter/lib/src/rules/no_leading_underscores_for_library_prefixes.dart:29-84`
    /// — an import prefix must not start with `_` (`hasLeadingUnderscore`,
    /// `pkg/linter/lib/src/util/ascii_utils.dart:58`), the `_` wildcard
    /// exempt when `wildcard_variables` is on (:46-50). Message:
    /// `diagnostic.g.dart` `noLeadingUnderscoresForLibraryPrefixes`.
    ///
    /// The official carries a second diagnostic variant (…`Shadowed`,
    /// :72-83) selected by whether stripping the underscore would rebind a
    /// prefix reference — an element-model question
    /// (`isShadowedAtSomeReference`). Both codes share identical
    /// problem/correction texts, so this spec renders both verbatim; the
    /// uniqueName split is unobservable at this face.
    pub(crate) static SPEC: RuleSpec = crate::spec!(
        "no_leading_underscores_for_library_prefixes",
        &[NodeKind::Import],
        check,
    );

    fn check(c: &mut Check<'_>) {
        let Some(prefix) = super::import_prefix(c, c.node()) else {
            return;
        };
        let name = c.lexeme(c.head_token(prefix));
        if c.unit.features.wildcard_variables && name == "_" {
            return;
        }
        if !crate::rules::has_leading_underscore(name) {
            return;
        }
        let range = c.utf16_span(prefix);
        c.report(range, &[name]);
    }
}

pub(crate) mod unnecessary_library_directive {
    use super::{Check, NodeKind, RuleSpec};

    /// Official: `upstream Dart SDK source, pkg/linter/lib/src/rules/unnecessary_library_directive.dart:38-50`
    /// — a `library` directive with no documentation comment and no
    /// annotations reports at the directive, unless the unit carries a
    /// `part` directive (:40-44 — parts may still use library names). The
    /// `sortedCommentAndAnnotations` face: a doc comment (`///` or `/**`)
    /// attached before the directive, or a `Metadata` on it, exempts; plain
    /// `//`/`/* */` comments do not. Message: `diagnostic.g.dart`
    /// `unnecessaryLibraryDirective`.
    pub(crate) static SPEC: RuleSpec = crate::spec!(
        "unnecessary_library_directive",
        &[NodeKind::LibraryName],
        check,
    );

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        // The directive's prelude wrapper sits one below the unit.
        let Some(unit) = c.ancestor(2) else {
            return;
        };
        if c.kind(unit) != NodeKind::CompilationUnit {
            return;
        }
        let kids = c.collect_direct(unit);
        let unwrapped: Vec<u32> = kids.iter().map(|&k| super::behind_prelude(c, k)).collect();
        // `parent.directives.any((e) => e is PartDirective)` (:40-44).
        if unwrapped.iter().any(|&k| c.kind(k) == NodeKind::Part) {
            return;
        }
        // The annotations: the declaration's star precedes the directive's
        // prelude among the unit children (parse order, `parseMetadataStar`
        // emits the star pair before the declaration opens) and carries
        // them.
        let Some(pos) = unwrapped.iter().position(|&k| k == node) else {
            return;
        };
        if pos > 0
            && c.kind(kids[pos - 1]) == NodeKind::MetadataStar
            && c.collect_direct(kids[pos - 1])
                .iter()
                .any(|&k| c.kind(k) == NodeKind::Metadata)
        {
            return;
        }
        // The documentation comment: a `///`/`/**` lexeme attached before
        // the directive's head token.
        if has_doc_comment(c, c.head_token(node)) {
            return;
        }
        let range = c.utf16_span(node);
        c.report(range, &[]);
    }

    /// Whether a documentation comment (`///` or `/**`) attaches to `token`
    /// (the scanner's documentation-comment lexemes; plain `//` and
    /// `/* */` never exempt).
    fn has_doc_comment(c: &Check<'_>, token: u32) -> bool {
        c.comments.attached(token as usize).iter().any(|comment| {
            let text = &c.unit.source[comment.range.start as usize..comment.range.end as usize];
            match comment.kind {
                crate::comments::CommentKind::Line => text.starts_with("///"),
                crate::comments::CommentKind::Block => text.starts_with("/**"),
            }
        })
    }

    #[cfg(test)]
    mod tests {
        use crate::testsupport::{Fixture, one_rule, tok, utf16_at};

        /// `test/rules/unnecessary_library_directive_test.dart`
        /// `test_unnecessary` — a bare directive reports over its whole
        /// span.
        #[test]
        fn reports_a_bare_library_directive() {
            let f = Fixture::parse("library lib;\n");
            let lints = f.run(&one_rule("unnecessary_library_directive"));
            assert_eq!(lints.len(), 1, "{lints:?}");
            assert_eq!(
                lints[0].message,
                "Library directives without comments or annotations should be avoided."
            );
            assert_eq!(
                lints[0].correction,
                Some("Try deleting the library directive.".to_owned())
            );
            let head = tok(&f.source, &f.tokens, "library", 0);
            assert_eq!(lints[0].range.start, utf16_at(&f, head));
        }

        /// `test_unnecessary` — the unnamed form reports the same way.
        #[test]
        fn reports_an_unnamed_library_directive() {
            let f = Fixture::parse("library;\n");
            assert_eq!(f.run(&one_rule("unnecessary_library_directive")).len(), 1);
        }

        /// `test_hasDocComment` — a doc comment exempts.
        #[test]
        fn ignores_a_directive_with_a_doc_comment() {
            let f = Fixture::parse("/// This is a nice library.\nlibrary with_comment;\n");
            assert!(f.run(&one_rule("unnecessary_library_directive")).is_empty());
        }

        /// A block doc comment exempts like the `///` form.
        #[test]
        fn ignores_a_block_doc_comment() {
            let f = Fixture::parse("/** Docs. */\nlibrary lib;\n");
            assert!(f.run(&one_rule("unnecessary_library_directive")).is_empty());
        }

        /// A plain `//` comment is not a documentation comment — the
        /// directive still reports (`sortedCommentAndAnnotations` carries
        /// doc comments and annotations only).
        #[test]
        fn reports_under_a_plain_comment() {
            let f = Fixture::parse("// Not a doc comment.\nlibrary lib;\n");
            assert_eq!(f.run(&one_rule("unnecessary_library_directive")).len(), 1);
        }

        /// `test_hasAnnotation` — an annotation exempts (the star's
        /// `Metadata` children).
        #[test]
        fn ignores_a_directive_with_an_annotation() {
            let f =
                Fixture::parse("@C()\nlibrary with_annotation;\n\nclass C {\n  const C();\n}\n");
            assert!(f.run(&one_rule("unnecessary_library_directive")).is_empty());
        }

        /// `test_hasPart` — a `part` directive in the unit keeps the
        /// library (parts may still use library names).
        #[test]
        fn ignores_a_unit_with_a_part_directive() {
            let f = Fixture::parse("library lib;\n\npart 'part.dart';\n");
            assert!(f.run(&one_rule("unnecessary_library_directive")).is_empty());
        }
    }
}

pub(crate) mod unnecessary_library_name {
    use super::{Check, NodeKind, PayloadField, RuleSpec};

    /// Official: `pkg/linter/lib/src/rules/unnecessary_library_name.dart:28-44`
    /// — a named `library` directive reports at the name. The rule registers
    /// only with the `unnamed_libraries` feature (shipped in 2.19,
    /// `tools/experimental_features.yaml` :397-400; :32-34). The name's
    /// presence is the `LibraryName.has_name` payload (the
    /// `end_library_name` back-fill). Message: `diagnostic.g.dart`
    /// `unnecessaryLibraryName`.
    pub(crate) static SPEC: RuleSpec =
        crate::spec!("unnecessary_library_name", &[NodeKind::LibraryName], check,);

    fn check(c: &mut Check<'_>) {
        if !c.unit.features.unnamed_libraries {
            return;
        }
        if c.field(c.node(), PayloadField::HasName).unwrap_or(0) == 0 {
            return;
        }
        // The name is the node's first child (Identifier or Qualified).
        let Some(name) = c.direct_children(c.node()).first().copied() else {
            return;
        };
        let range = c.utf16_span(name);
        c.report(range, &[]);
    }
}

pub(crate) mod use_string_in_part_of_directives {
    use super::{Check, NodeKind, PayloadField, RuleSpec};

    /// Official: `pkg/linter/lib/src/rules/use_string_in_part_of_directives.dart:29-47`
    /// — a `part of` in the dotted-name form reports at the directive. The
    /// rule registers only without the `enhanced_parts` feature (experimental,
    /// unshipped — `enhanced-parts` has no `enabledIn`,
    /// `tools/experimental_features.yaml` :150-152; :32-34). The form is the
    /// `PartOf.has_name` payload (the `end_part_of` back-fill).
    /// Message: `diagnostic.g.dart` `useStringInPartOfDirectives`.
    pub(crate) static SPEC: RuleSpec = crate::spec!(
        "use_string_in_part_of_directives",
        &[NodeKind::PartOf],
        check,
    );

    fn check(c: &mut Check<'_>) {
        if c.unit.features.enhanced_parts {
            return;
        }
        if c.field(c.node(), PayloadField::HasName).unwrap_or(0) == 0 {
            return;
        }
        let range = c.utf16_span(c.node());
        c.report(range, &[]);
    }
}

pub(crate) mod directives_ordering {
    use super::{Check, NodeKind, RuleSpec, written_uri};

    /// Official: `pkg/linter/lib/src/rules/directives_ordering.dart:120-268`
    /// — four checks over the unit's directives, each node reporting at
    /// most once across all of them (the `lintedNodes` set): `dart:` URIs
    /// go before other directives of their kind (`withDartUrisSkipping
    /// TheFirstSet`, :259-268), `package:` URIs go before relative ones
    /// after the first run of absolute URIs (:270-280), exports come after
    /// all imports (:196-210 — trailing `part`s then trailing exports
    /// skipped from the end), and the <dart:/relative/package> sections are
    /// each neighbor-sorted by [`compare_directives`] (:180-194 over
    /// :212-257). URIs are the cooked adjacent-string `stringValue`; a
    /// null value (interpolation) excludes the directive from the
    /// dart/package sections and pauses the alphabetical comparison while
    /// staying in the relative one (`isRelativeDirective` is the
    /// not-absolute face). The `@docImport` faces (:136-155) are outside
    /// this slice: doc imports are doc-comment directives the dartq parser
    /// does not model (an analyzer-only feature, off by default), so the
    /// oracle's default-config parity is unaffected. Message variants:
    /// `diagnostic.g.dart` `directivesOrderingDart`,
    /// `directivesOrderingPackageBeforeRelative`,
    /// `directivesOrderingExports`, `directivesOrderingAlphabetical` (the
    /// nominal spec face; every report carries its own texts).
    pub(crate) static SPEC: RuleSpec =
        crate::spec!("directives_ordering", &[NodeKind::CompilationUnit], check,);

    const DART_PROBLEM: &str = "Place 'dart:' {0} before other {0}.";
    const PACKAGE_PROBLEM: &str = "Place 'package:' {0} before relative {0}.";
    const EXPORTS_PROBLEM: &str = "Specify exports in a separate section after all imports.";
    const CORRECTION: &str = "Try sorting the directives.";

    fn check(c: &mut Check<'_>) {
        let directives: Vec<u32> = c
            .collect_direct(c.node())
            .into_iter()
            .map(|kid| super::behind_prelude(c, kid))
            .filter(|&kid| {
                matches!(
                    c.kind(kid),
                    NodeKind::Import
                        | NodeKind::Export
                        | NodeKind::Part
                        | NodeKind::PartOf
                        | NodeKind::LibraryName
                )
            })
            .collect();
        let mut linted: Vec<u32> = Vec::new();
        check_dart_first(c, &directives, &mut linted);
        check_package_before_relative(c, &directives, &mut linted);
        check_exports_after_imports(c, &directives, &mut linted);
        check_sections_alphabetical(c, &directives, &mut linted);
    }

    fn report(c: &mut Check<'_>, directive: u32, problem: &str, args: &[&str]) {
        let span = c.utf16_span(directive);
        c.report_texts(span, problem, CORRECTION, args);
    }

    /// The marks of the `lintedNodes` set: `true` when `directive` reports.
    fn mark(linted: &mut Vec<u32>, directive: u32) -> bool {
        if linted.contains(&directive) {
            return false;
        }
        linted.push(directive);
        true
    }

    fn section(c: &Check<'_>, directives: &[u32], kind: NodeKind) -> Vec<u32> {
        directives
            .iter()
            .copied()
            .filter(|&d| c.kind(d) == kind)
            .collect()
    }

    fn check_dart_first(c: &mut Check<'_>, directives: &[u32], linted: &mut Vec<u32>) {
        for kind in [NodeKind::Import, NodeKind::Export] {
            let label = kind_label(kind);
            // `withDartUrisSkippingTheFirstSet`: skip the leading dart: run,
            // then every further dart: directive reports.
            let mut past_run = false;
            for directive in section(c, directives, kind) {
                let uri = super::written_uri(c, directive).map(|(_, value)| value);
                if !past_run {
                    if is_dart(uri.as_ref()) {
                        continue;
                    }
                    past_run = true;
                }
                if is_dart(uri.as_ref()) && mark(linted, directive) {
                    report(c, directive, DART_PROBLEM, &[label, label]);
                }
            }
        }
    }

    fn check_package_before_relative(c: &mut Check<'_>, directives: &[u32], linted: &mut Vec<u32>) {
        for kind in [NodeKind::Import, NodeKind::Export] {
            let label = kind_label(kind);
            // `withPackageUrisSkippingAbsoluteUris`: drop dart:, skip the
            // leading absolute run, then every package: directive reports.
            let mut past_run = false;
            for directive in section(c, directives, kind) {
                let uri = super::written_uri(c, directive).map(|(_, value)| value);
                if is_dart(uri.as_ref()) {
                    continue;
                }
                if !past_run {
                    if is_absolute(uri.as_ref()) {
                        continue;
                    }
                    past_run = true;
                }
                if is_package(uri.as_ref()) && mark(linted, directive) {
                    report(c, directive, PACKAGE_PROBLEM, &[label, label]);
                }
            }
        }
    }

    fn check_exports_after_imports(c: &mut Check<'_>, directives: &[u32], linted: &mut Vec<u32>) {
        // `reversed.skipWhile(isPart).skipWhile(isExport)`: trailing parts
        // then trailing exports are exempt; every export before them
        // reports.
        let mut end = directives.len();
        while end > 0 && c.kind(directives[end - 1]) == NodeKind::Part {
            end -= 1;
        }
        while end > 0 && c.kind(directives[end - 1]) == NodeKind::Export {
            end -= 1;
        }
        for &directive in &directives[..end] {
            if c.kind(directive) == NodeKind::Export && mark(linted, directive) {
                report(c, directive, EXPORTS_PROBLEM, &[]);
            }
        }
    }

    fn check_sections_alphabetical(c: &mut Check<'_>, directives: &[u32], linted: &mut Vec<u32>) {
        // The section walk order (:219-253): dart, relative, then package —
        // imports before exports within each. All projects count as "not
        // pub" upstream (`DartProject` removal, :201-208), so package
        // directives form one section.
        for (predicate, kind) in [
            (is_dart as UriPredicate, NodeKind::Import),
            (is_dart as UriPredicate, NodeKind::Export),
            (is_relative as UriPredicate, NodeKind::Import),
            (is_relative as UriPredicate, NodeKind::Export),
            (is_package as UriPredicate, NodeKind::Import),
            (is_package as UriPredicate, NodeKind::Export),
        ] {
            let nodes: Vec<u32> = section(c, directives, kind)
                .into_iter()
                .filter(|&d| predicate(uri_of(c, d).as_ref()))
                .collect();
            let mut previous = nodes.first().and_then(|&d| uri_of(c, d));
            for directive in nodes.into_iter().skip(1) {
                let current = uri_of(c, directive);
                if let (Some(prev), Some(cur)) = (previous.as_deref(), current.as_deref())
                    && compare_directives(prev, cur) == core::cmp::Ordering::Greater
                    && mark(linted, directive)
                {
                    report(c, directive, SPEC.problem, &[]);
                }
                previous = current;
            }
        }
    }

    type UriPredicate = fn(Option<&String>) -> bool;

    fn uri_of(c: &Check<'_>, directive: u32) -> Option<String> {
        written_uri(c, directive).map(|(_, value)| value)
    }

    fn kind_label(kind: NodeKind) -> &'static str {
        if kind == NodeKind::Import {
            "imports"
        } else {
            "exports"
        }
    }

    fn is_dart(uri: Option<&String>) -> bool {
        uri.is_some_and(|value| value.starts_with("dart:"))
    }

    /// `_isAbsoluteDirective`: a readable URI containing `:` — a null value
    /// (`stringValue` on interpolation) is never absolute.
    fn is_absolute(uri: Option<&String>) -> bool {
        uri.is_some_and(|value| value.contains(':'))
    }

    fn is_package(uri: Option<&String>) -> bool {
        uri.is_some_and(|value| value.starts_with("package:"))
    }

    /// `_isRelativeDirective` is the not-absolute face: a null URI joins
    /// the relative section.
    fn is_relative(uri: Option<&String>) -> bool {
        !is_absolute(uri)
    }

    /// `compareDirectives` (:36-50): package-to-package (or any pair
    /// involving a `/`-absolute path) compares the package segment first
    /// and the remainder second; every other pair compares whole. The
    /// string order is code-unit order (`String.compareTo`), which the
    /// `str` order mirrors for the ASCII URIs directives carry.
    fn compare_directives(a: &str, b: &str) -> core::cmp::Ordering {
        if (!a.starts_with("package:") || !b.starts_with("package:"))
            && !a.starts_with('/')
            && !b.starts_with('/')
        {
            return a.cmp(b);
        }
        let (Some(index_a), Some(index_b)) = (a.find('/'), b.find('/')) else {
            return a.cmp(b);
        };
        a[..index_a]
            .cmp(&b[..index_b])
            .then_with(|| a[index_a + 1..].cmp(&b[index_b + 1..]))
    }
}

pub(crate) mod library_names {
    use super::{Check, NodeKind, PayloadField, RuleSpec};

    /// Official: `pkg/linter/lib/src/rules/library_names.dart:28-46` — a
    /// named `library` directive whose name is not
    /// `lowercase_with_underscores` reports at the whole name (dotted
    /// names report across every segment, `library one.Two.three` in the
    /// official corpus). The validator is `utils.dart:35`
    /// `_lowerCaseUnderScoreWithDots`: `^_?[_a-z\d]*(?:\.[a-z][_a-z\d]*)*$`.
    /// Message: `diagnostic.g.dart` `libraryNames`.
    pub(crate) static SPEC: RuleSpec =
        crate::spec!("library_names", &[NodeKind::LibraryName], check,);

    fn check(c: &mut Check<'_>) {
        if c.field(c.node(), PayloadField::HasName).unwrap_or(0) == 0 {
            return;
        }
        // The name pieces are the node's children — the identifiers plus
        // the childless `Qualified` markers between them — so the dotted
        // text and the report anchor are the children's envelope (the
        // official `name.toString()` face; `library one.Two.three` in the
        // official corpus reports across every segment).
        let children = c.collect_direct(c.node());
        let Some(&head) = children.first() else {
            return;
        };
        let mut bytes = c.byte_span(head);
        let mut utf16 = c.utf16_span(head);
        for &kid in &children[1..] {
            let kid_bytes = c.byte_span(kid);
            bytes.start = bytes.start.min(kid_bytes.start);
            bytes.end = bytes.end.max(kid_bytes.end);
            let kid_utf16 = c.utf16_span(kid);
            utf16.start = utf16.start.min(kid_utf16.start);
            utf16.end = utf16.end.max(kid_utf16.end);
        }
        let text = &c.unit.source[bytes];
        if is_lower_case_under_score_with_dots(text) {
            return;
        }
        c.report(utf16, &[text]);
    }

    /// `utils.dart:35` — one optional leading underscore, a
    /// `[_a-z\d]*` run, then dot-separated `[a-z][_a-z\d]*` segments.
    fn is_lower_case_under_score_with_dots(id: &str) -> bool {
        let b = id.as_bytes();
        let mut i = usize::from(b.first() == Some(&b'_'));
        while i < b.len() && (b[i] == b'_' || b[i].is_ascii_lowercase() || b[i].is_ascii_digit()) {
            i += 1;
        }
        while i < b.len() {
            if b[i] != b'.' {
                return false;
            }
            i += 1;
            if !b.get(i).is_some_and(|&c| c.is_ascii_lowercase()) {
                return false;
            }
            i += 1;
            while i < b.len()
                && (b[i] == b'_' || b[i].is_ascii_lowercase() || b[i].is_ascii_digit())
            {
                i += 1;
            }
        }
        true
    }
}

pub(crate) mod package_prefixed_library_names {
    use super::{Check, NodeKind, RuleSpec};

    /// Official: `pkg/linter/lib/src/rules/package_prefixed_library_names.dart`
    /// — registered but permanently silent: the upstream visitor body is
    /// `return;` ("Project info is not being set", the `DartProject`
    /// removal tracked as dart-lang/linter#3395; the intended prefix walk
    /// survives only as a comment). The oracle emits nothing for this rule,
    /// so parity is the no-op — a live reimplementation would fire where
    /// `dart analyze` stays silent. The spec carries the registered texts
    /// (`diagnostic.g.dart` `packagePrefixedLibraryNames`) for the listing
    /// face.
    pub(crate) static SPEC: RuleSpec = crate::spec!(
        "package_prefixed_library_names",
        &[NodeKind::LibraryName],
        check,
    );

    fn check(_c: &mut Check<'_>) {}
}

pub(crate) mod provide_deprecation_message {
    use super::{Check, NodeKind, RuleSpec};

    /// Official: `pkg/linter/lib/src/rules/provide_deprecation_message.dart:32-42`
    /// — an annotation whose element is the dart:core `deprecated` constant
    /// and which carries no argument list reports at the annotation.
    ///
    /// dartq faces: the element pin is taken from the written name — a bare
    /// `deprecated` (bailing when a same-file declaration shadows the
    /// name), or an import-qualified `foo.deprecated` whose prefix binds
    /// dart:core through a same-file import
    /// ([`prefixed_dart_core_name`]). `@Deprecated` — the class, not the
    /// constant — stays silent per the element pin (`isDeprecated`,
    /// :37-42); the oracle reports it only under a compile error (a bare
    /// class annotation is always `no_annotation_constructor_arguments`), a
    /// face unreachable on valid code. Message:
    /// `diagnostic.g.dart` `provideDeprecationMessage`.
    pub(crate) static SPEC: RuleSpec =
        crate::spec!("provide_deprecation_message", &[NodeKind::Metadata], check,);

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        let kids = c.collect_direct(node);
        // `node.arguments == null` (:40). A bare `@deprecated` emits
        // `NoArguments`; `@deprecated()` keeps a real one and passes.
        if kids.iter().any(|&kid| c.kind(kid) == NodeKind::Arguments) {
            return;
        }
        // `elementAnnotation.isDeprecated` (:37-42): the dart:core
        // `deprecated` constant — bare, or import-qualified through a
        // same-file prefix.
        let Some((prefix, name)) = super::metadata_name_face(c, node) else {
            return;
        };
        if c.lexeme(c.head_token(name)) != "deprecated" {
            return;
        }
        match prefix {
            None => {
                if crate::rules::expressions::same_file::declares_name(c, "deprecated") {
                    return;
                }
            }
            Some(prefix) => {
                if !super::prefixed_dart_core_name(c, c.lexeme(c.head_token(prefix)), "deprecated")
                {
                    return;
                }
            }
        }
        let range = c.utf16_span(node);
        c.report(range, &[]);
    }
}

pub(crate) mod implementation_imports {
    //! Official: `pkg/linter/lib/src/rules/implementation_imports.dart` —
    //! an import whose resolved URI is a `package:<other>/src/…`
    //! implementation path (`Uri.isImplementation`,
    //! `pkg/analyzer/lib/utilities/extensions/uri.dart:8-11`) of a package
    //! other than the analyzing library's own (`isSamePackageAs`,
    //! uri.dart:15-21) reports at the import's URI node (:44-56).
    //!
    //! Mounted on the AST dispatch off
    //! [`WorkspaceFacts`][crate::WorkspaceFacts] (`Unit.workspace`): the
    //! official registration gate is the analyzing library's `package:`
    //! URI (:30-33) — mounted as the file sitting under its package's
    //! package-URI directory (`lib_dir`). Under that gate a written
    //! `package:` URI resolves to itself; every other written form
    //! resolves to a non-`package:` URI (`file:` absolute path, `dart:`
    //! short path) that fails `isImplementation`, so the written-URI
    //! verdict is the resolved one. The documented divergence: a written
    //! `package:` URI that resolves to nothing (a package outside the
    //! config) reports here where the official's `DirectiveUriWithSource`
    //! guard stays silent — unresolvable imports are erroneous code.

    use super::{Check, NodeKind, RuleSpec, written_uri};

    pub(crate) static SPEC: RuleSpec =
        crate::spec!("implementation_imports", &[NodeKind::Import], check,);

    fn check(c: &mut Check<'_>) {
        let Some(facts) = c.unit.workspace else {
            return;
        };
        // The official gate is the analyzing library's `package:` URI
        // (:30-33); parts resolve through their library, which a part file
        // outside `lib_dir` of a `lib/` library under-reports (parts live
        // beside their library in practice).
        if c.unit
            .path
            .is_none_or(|path| !path.starts_with(&facts.lib_dir))
        {
            return;
        }
        let Some((range, uri)) = written_uri(c, c.node()) else {
            return;
        };
        if !uri.starts_with("package:") {
            return;
        }
        if check_import(&facts.own_package, &uri) {
            c.report(range, &[]);
        }
    }

    /// The official `visitImportDirective` verdict (:44-56) over one
    /// import: `true` reports at the URI node. `own_package` is the first
    /// path segment of the analyzing library's `package:` URI (its package
    /// name); `uri` is the import's resolved URI (the official
    /// `importedLibrary.source.uri` — a `package:` or `file:` form, not
    /// the written string).
    #[must_use]
    pub fn check_import(own_package: &str, uri: &str) -> bool {
        uri_is_implementation(uri) && !uri_is_same_package_as(own_package, uri)
    }

    /// `Uri.isImplementation` (uri.dart:8-11):
    /// `(isScheme('package') || !hasAbsolutePath) && >2 path segments &&
    /// segments[1] == 'src'`.
    #[must_use]
    pub fn uri_is_implementation(uri: &str) -> bool {
        let (scheme, path) = scheme_and_path(uri);
        if !scheme.eq_ignore_ascii_case("package") && path.starts_with('/') {
            return false;
        }
        let segments: Vec<&str> = path.split('/').collect();
        segments.len() > 2 && segments[1] == "src"
    }

    /// `Uri.isSamePackageAs` (uri.dart:15-21) with the source URI reduced
    /// to its package name: both sides `package:` URIs with a non-empty
    /// segment list and equal first segments.
    #[must_use]
    pub fn uri_is_same_package_as(own_package: &str, uri: &str) -> bool {
        let (scheme, path) = scheme_and_path(uri);
        scheme.eq_ignore_ascii_case("package")
            // `pathSegments.isNotEmpty`: a `package:` URI always yields at
            // least one segment (the empty path's `[""]`).
            && path.split('/').next() == Some(own_package)
    }

    /// The scheme and path of a URI string (the `Uri.tryParse` +
    /// `.scheme`/`.path` face): the RFC 3986 scheme prefix (empty when
    /// absent; returned as written — the call sites compare
    /// ASCII-case-insensitively, covering Dart's lowercase scheme
    /// normalization), and the remainder with any `?query`/`#fragment`
    /// stripped. Segmenting mirrors `pathSegments`: `'/'`-split, leading
    /// empty segment on absolute paths, trailing empty on a trailing
    /// slash.
    fn scheme_and_path(uri: &str) -> (&str, &str) {
        let b = uri.as_bytes();
        let mut scheme_end = 0;
        if !b.is_empty() && b[0].is_ascii_alphabetic() {
            let mut i = 1;
            while i < b.len()
                && (b[i].is_ascii_alphanumeric() || matches!(b[i], b'+' | b'-' | b'.'))
            {
                i += 1;
            }
            if i < b.len() && b[i] == b':' {
                scheme_end = i;
            }
        }
        let rest = &uri[scheme_end..];
        let rest = rest.strip_prefix(':').unwrap_or(rest);
        // `Uri.path` excludes the query and fragment.
        let path_end = rest.find(['?', '#']).unwrap_or(rest.len());
        let scheme = if scheme_end > 0 {
            &uri[..scheme_end]
        } else {
            ""
        };
        (scheme, &rest[..path_end])
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// The official `test_differentPackage_src`.
        #[test]
        fn other_package_src_reports() {
            assert!(check_import("test", "package:foo/src/foo.dart"));
        }

        /// `test_samePackage_src` — own-package `src/` imports are legal.
        #[test]
        fn own_package_src_is_silent() {
            assert!(!check_import("foo", "package:foo/src/foo.dart"));
        }

        /// `test_differentPackage_lib` — public libraries never report.
        #[test]
        fn public_library_is_silent() {
            assert!(!check_import("test", "package:foo/foo.dart"));
        }

        /// A resolved `file:` URI has an absolute path — the
        /// `!hasAbsolutePath` arm of `isImplementation` fails.
        #[test]
        fn resolved_file_uris_are_silent() {
            assert!(!check_import("test", "file:///pkg/lib/src/foo.dart"));
        }

        /// `isImplementation` needs more than two path segments.
        #[test]
        fn segment_count_gates() {
            assert!(!check_import("test", "package:foo/src"));
            // A trailing slash yields a final empty segment
            // (`Uri.pathSegments`, uri.dart:4700-4715), so the gate holds —
            // such a URI stays silent only because it never resolves to a
            // source (the official `DirectiveUriWithSource` guard is the
            // mounter's premise, not the predicate's).
            assert!(check_import("test", "package:foo/src/"));
            assert!(check_import("test", "package:foo/src/a.dart"));
        }

        /// The `Uri.path` face strips query and fragment before
        /// segmentation.
        #[test]
        fn query_and_fragment_strip() {
            assert!(check_import("test", "package:foo/src/a.dart?x=1#f"));
        }

        #[test]
        fn same_package_predicate_needs_the_package_scheme() {
            assert!(uri_is_same_package_as("foo", "package:foo/src/a.dart"));
            assert!(!uri_is_same_package_as("foo", "package:bar/src/a.dart"));
            assert!(!uri_is_same_package_as("foo", "file:///x/lib/src/a.dart"));
        }
    }
}

pub(crate) mod combinators_ordering {
    use super::{Check, NodeKind, RuleSpec};

    /// Official: `pkg/linter/lib/src/rules/combinators_ordering.dart:35-50`
    /// — a `show`/`hide` combinator whose names are not sorted reports at
    /// the combinator. `isSorted` is the package:collection default —
    /// case-sensitive code-unit order (`String.compareTo`). Message:
    /// `diagnostic.g.dart` `combinatorsOrdering`.
    pub(crate) static SPEC: RuleSpec = crate::spec!(
        "combinators_ordering",
        &[NodeKind::Show, NodeKind::Hide],
        check,
    );

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        // The names are the combinator's `Identifier` children (the
        // list delimits itself).
        let names: Vec<&str> = c
            .collect_direct(node)
            .into_iter()
            .filter(|&kid| c.kind(kid) == NodeKind::Identifier)
            .map(|kid| c.lexeme(c.head_token(kid)))
            .collect();
        if names.windows(2).any(|pair| pair[0] > pair[1]) {
            c.report(c.utf16_span(node), &[]);
        }
    }
}

pub(crate) mod migrate_design_widgets {
    use super::{Check, NodeKind, RuleSpec, written_uri};

    /// Official: `pkg/linter/lib/src/rules/migrate_design_widgets.dart:34-43`
    /// — an import of the deprecated design libraries
    /// (`package:flutter/material.dart`,
    /// `package:flutter/cupertino.dart`) reports at the URI with the
    /// library spelled in the message. Message: `diagnostic.g.dart`
    /// `migrateDesignWidgets`.
    pub(crate) static SPEC: RuleSpec =
        crate::spec!("migrate_design_widgets", &[NodeKind::Import], check,);

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        if let Some((span, value)) = written_uri(c, node).filter(|(_, value)| {
            matches!(
                value.as_str(),
                "package:flutter/material.dart" | "package:flutter/cupertino.dart"
            )
        }) {
            c.report(span, &[&value]);
        }
    }
}

pub(crate) mod always_use_package_imports {
    //! Official: `upstream Dart SDK source, pkg/linter/lib/src/rules/always_use_package_imports.dart`
    //! — an import whose URI has an empty scheme (`isRelativeImport`,
    //! :43-51: `Uri.tryParse(uri.stringValue)`, `uri.scheme.isEmpty`)
    //! reports at the URI (:53-56). The rule registers only inside the
    //! package's `lib/` (:35 — relative paths elsewhere are
    //! `avoid_relative_lib_imports`' domain; the gate is
    //! `_isInLibDir`, `pkg/analyzer/lib/src/analysis_rule/rule_context.dart:16-21`).
    //! Mounted on the AST dispatch off
    //! [`WorkspaceFacts`][crate::WorkspaceFacts]: `workspace: None` is the
    //! official no-package registration gate, and the lib/ containment is
    //! the same `starts_with(lib_dir)` face `implementation_imports`
    //! ports. Message: `diagnostic.g.dart` `alwaysUsePackageImports`.

    use super::{Check, NodeKind, RuleSpec, written_uri};

    pub(crate) static SPEC: RuleSpec =
        crate::spec!("always_use_package_imports", &[NodeKind::Import], check,);

    fn check(c: &mut Check<'_>) {
        let Some(facts) = c.unit.workspace else {
            return;
        };
        // `context.isInLibDir` (rule_context.dart:16-21): the analyzed file
        // sits under the package's package-URI directory.
        if c.unit
            .path
            .is_none_or(|path| !path.starts_with(&facts.lib_dir))
        {
            return;
        }
        let Some((range, uri)) = written_uri(c, c.node()) else {
            return;
        };
        // `Uri.tryParse(uri).scheme.isEmpty` (:43-51) — the scheme walk
        // `avoid_relative_lib_imports` ports (:28-32); an interpolated URI
        // (null `stringValue`) never reaches here.
        if super::avoid_relative_lib_imports::scheme_prefix_len(&uri).is_none() {
            c.report(range, &[]);
        }
    }
}

pub(crate) mod prefer_relative_imports {
    //! Official: `upstream Dart SDK source, pkg/linter/lib/src/rules/prefer_relative_imports.dart`
    //! — a `package:` self-reference import reports at the URI (:50-65,
    //! :67-70): the import's resolved URI is a `package:` URI of the
    //! analyzing library's own package (`isSamePackageAs`,
    //! `pkg/analyzer/lib/utilities/extensions/uri.dart:15-21`) whose source
    //! sits under the package root, registered only inside `lib/` (:35).
    //! Mounted on the AST dispatch off
    //! [`WorkspaceFacts`][crate::WorkspaceFacts] under the
    //! [`implementation_imports`][super::implementation_imports] premise:
    //! under the lib/ gate a written `package:` URI resolves to itself and
    //! every other written form resolves to an off-package URI, so the
    //! written-URI verdict is the resolved one (the root-containment
    //! conjunct rides the gate — an own-package `package:` URI maps under
    //! the package root). Documented divergence: a written own-package URI
    //! that resolves to nothing reports where the official's
    //! `DirectiveUriWithSource` guard stays silent — unresolvable imports
    //! are erroneous code. Message: `diagnostic.g.dart`
    //! `preferRelativeImports`.

    use super::{Check, NodeKind, RuleSpec, written_uri};

    pub(crate) static SPEC: RuleSpec =
        crate::spec!("prefer_relative_imports", &[NodeKind::Import], check,);

    fn check(c: &mut Check<'_>) {
        let Some(facts) = c.unit.workspace else {
            return;
        };
        // `context.isInLibDir` (:35).
        if c.unit
            .path
            .is_none_or(|path| !path.starts_with(&facts.lib_dir))
        {
            return;
        }
        let Some((range, uri)) = written_uri(c, c.node()) else {
            return;
        };
        // `importUri.isScheme('package') && isSamePackageAs(sourceUri)`
        // (:50-65) over the written face.
        if super::implementation_imports::uri_is_same_package_as(&facts.own_package, &uri) {
            c.report(range, &[]);
        }
    }
}

pub(crate) mod simple_directive_paths {
    //! Official: `upstream Dart SDK source, pkg/linter/lib/src/rules/simple_directive_paths.dart`
    //! — every directive URI reports unless it already is the
    //! `Uri.parse(uri).toString()` form (:73-104): dot segments and
    //! unnecessary percent-escapes collapse (`./a.dart`, `a/../b.dart`,
    //! `%41.dart`), and a scheme-less path must be the minimal relative
    //! path from the containing unit's directory
    //! (`path.url.relative(resolved.path, from: dirname(unit))`, :97-103).
    //! The walk covers imports, exports, parts, the URI form of `part of`
    //! (:51-70), and each conditional-uri candidate (:46-48); the face is
    //! the cooked adjacent-string `stringValue` (interpolation never
    //! registers — `uriString == null`, :74-77).
    //!
    //! The normalization mirrors `sdk/lib/core/uri.dart`'s parse-time path
    //! face, in its decision order: escape normalization first
    //! (`_makePath` — unreserved escapes decode, escape hex digits
    //! uppercase, a stray `%` re-encodes to `%25`, disallowed characters
    //! percent-encode their UTF-8 bytes, `\` becomes `/`), then
    //! dot-segment removal over the escaped path (`_normalizePath` — the
    //! pure-relative face drops `.` and non-leading `..` segments keeping
    //! the leading ones, `_normalizeRelativePath`; every other face
    //! follows the RFC remove-dot-segments walk, `_removeDotSegments`).
    //! Faces left out, both under-reporting: query/fragment components
    //! stay verbatim (the official normalizes escapes there too), and the
    //! `_escapeScheme` lookalike-scheme re-escape at the relative face's
    //! tail is skipped (reachable only through encoded-colon spellings
    //! like `a%2E:b`). Message: `diagnostic.g.dart` `simpleDirectivePaths`.

    use super::{Check, NodeKind, RuleSpec, candidate_uri, written_uri};

    pub(crate) static SPEC: RuleSpec = crate::spec!(
        "simple_directive_paths",
        &[
            NodeKind::Import,
            NodeKind::Export,
            NodeKind::Part,
            NodeKind::PartOf,
            NodeKind::ConditionalUri,
        ],
        check,
    );

    fn check(c: &mut Check<'_>) {
        // The directive's primary URI; on a conditional-uri clause the
        // configured candidate (an `==` comparison value precedes it in
        // the arena).
        let read = if c.kind(c.node()) == NodeKind::ConditionalUri {
            candidate_uri(c, c.node())
        } else {
            written_uri(c, c.node())
        };
        let Some((range, uri)) = read else {
            return;
        };
        let dir = c
            .unit
            .path
            .and_then(|path| path.parent())
            .map(|dir| dir.to_string_lossy().into_owned());
        if reports(dir.as_deref(), &uri) {
            c.report(range, &[]);
        }
    }

    /// The official `_check` verdict (:73-104) over one written URI from a
    /// unit whose directory is `unit_dir` (`None` — a pathless unit, the
    /// official `contextUri == null` bail): `true` reports at the URI
    /// node. Paths are canonical `/`-separated strings.
    #[must_use]
    pub fn reports(unit_dir: Option<&str>, uri: &str) -> bool {
        if uri.is_empty() {
            return false;
        }
        let (scheme, authority, path, tail) = split_uri(uri);
        // `_makePath` + `_normalizePath`: escapes first, then dot segments
        // over the escaped path.
        let escaped = escape_normalize(path);
        let pure_relative = scheme.is_none() && authority.is_none() && !escaped.starts_with('/');
        let normalized = if pure_relative {
            normalize_relative(&escaped)
        } else {
            remove_dot_segments(&escaped)
        };
        let mut rebuilt = String::with_capacity(uri.len());
        if let Some(scheme) = &scheme {
            rebuilt.push_str(scheme);
            rebuilt.push(':');
        }
        if let Some(authority) = authority {
            rebuilt.push_str("//");
            rebuilt.push_str(authority);
        }
        rebuilt.push_str(&normalized);
        rebuilt.push_str(tail);
        if rebuilt != uri {
            return true;
        }
        // The minimal-relative face (:92-103): scheme-less, no authority,
        // not an absolute path, non-empty path.
        if !pure_relative || normalized.is_empty() {
            return false;
        }
        let Some(dir) = unit_dir else {
            return false;
        };
        // `contextUri.resolveUri(parsedUri)` — the resolved path is
        // dot-free.
        let joined = if dir.is_empty() {
            normalized
        } else {
            format!("{dir}/{normalized}")
        };
        minimal_relative(dir, &remove_dot_segments(&joined)) != uri
    }

    /// The structural `Uri.tryParse` split of one URI string: the
    /// lowercased scheme (`None` when scheme-less), the verbatim authority
    /// (`None` when absent), the path slice (no query/fragment), and the
    /// verbatim `?`/`#` tail.
    fn split_uri(uri: &str) -> (Option<String>, Option<&str>, &str, &str) {
        let (scheme, rest) = match super::avoid_relative_lib_imports::scheme_prefix_len(uri) {
            Some(len) => (Some(uri[..len - 1].to_ascii_lowercase()), &uri[len..]),
            None => (None, uri),
        };
        let (authority, rest) = match rest.strip_prefix("//") {
            Some(after) => {
                let end = after.find(['/', '?', '#']).unwrap_or(after.len());
                (Some(&after[..end]), &after[end..])
            }
            None => (None, rest),
        };
        let path_end = rest.find(['?', '#']).unwrap_or(rest.len());
        (scheme, authority, &rest[..path_end], &rest[path_end..])
    }

    /// The `_pathCharOrSlashMask` escape normalization (`_makePath`):
    /// allowed characters pass, an unreserved escape decodes, remaining
    /// escapes uppercase their hex digits, a stray `%` re-encodes to
    /// `%25`, `\` becomes `/`, and everything else percent-encodes its
    /// UTF-8 bytes.
    fn escape_normalize(path: &str) -> String {
        use core::fmt::Write as _;

        let bytes = path.as_bytes();
        let is_allowed = |byte: u8| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'-' | b'.'
                        | b'_'
                        | b'~'
                        | b'!'
                        | b'$'
                        | b'&'
                        | b'\''
                        | b'('
                        | b')'
                        | b'*'
                        | b'+'
                        | b','
                        | b';'
                        | b'='
                        | b':'
                        | b'@'
                        | b'/'
                )
        };
        let is_unreserved =
            |byte: u8| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~');
        let mut out = String::with_capacity(path.len());
        let mut run = 0;
        let mut i = 0;
        while i < bytes.len() {
            if is_allowed(bytes[i]) {
                i += 1;
                continue;
            }
            out.push_str(&path[run..i]);
            match bytes[i] {
                b'%' => {
                    let decoded = bytes.get(i + 1..i + 3).and_then(|hex| {
                        let hi = (hex[0] as char).to_digit(16)?;
                        let lo = (hex[1] as char).to_digit(16)?;
                        u8::try_from(hi * 16 + lo).ok()
                    });
                    match decoded {
                        Some(value) if is_unreserved(value) => out.push(value as char),
                        // String's `fmt::Write` is infallible.
                        Some(value) => {
                            let _ = core::write!(out, "%{value:02X}");
                        }
                        None => out.push_str("%25"),
                    }
                    i += if decoded.is_some() { 3 } else { 1 };
                }
                b'\\' => {
                    out.push('/');
                    i += 1;
                }
                _ => {
                    // One UTF-8 sequence — every byte encodes.
                    let end = (i + utf8_sequence_len(bytes[i])).min(bytes.len());
                    for &byte in &bytes[i..end] {
                        // String's `fmt::Write` is infallible.
                        let _ = core::write!(out, "%{byte:02X}");
                    }
                    i = end;
                }
            }
            run = i;
        }
        out.push_str(&path[run..]);
        out
    }

    /// The UTF-8 sequence length of a character from its head byte.
    /// Continuation bytes never start a walk (slices sit at character
    /// boundaries); a stray one encodes alone.
    fn utf8_sequence_len(head: u8) -> usize {
        match head {
            0xC0..=0xDF => 2,
            0xE0..=0xEF => 3,
            0xF0..=0xF7 => 4,
            _ => 1,
        }
    }

    /// `Uri._normalizeRelativePath`: drops every `.` segment and every
    /// non-leading `..` segment, keeps leading `..`s, preserves a trailing
    /// slash, and keeps an emptied path visible as `./`.
    fn normalize_relative(path: &str) -> String {
        let mut output: Vec<String> = Vec::new();
        let mut append_slash = false;
        for segment in path.split('/') {
            if segment == ".." {
                append_slash = true;
                if output.last().is_some_and(|last| last != "..") {
                    output.pop();
                } else {
                    output.push("..".to_owned());
                }
            } else if segment == "." {
                append_slash = true;
            } else {
                append_slash = false;
                if segment.is_empty() && output.is_empty() {
                    // A leading empty segment stays relative as `./`.
                    output.push("./".to_owned());
                } else {
                    output.push(segment.to_owned());
                }
            }
        }
        if output.is_empty() {
            return "./".to_owned();
        }
        if append_slash {
            output.push(String::new());
        }
        output.join("/")
    }

    /// `Uri._removeDotSegments` — the RFC face, treating the path as
    /// absolute (leading `..` clamps at the root).
    fn remove_dot_segments(path: &str) -> String {
        let mut output: Vec<&str> = Vec::new();
        let mut append_slash = false;
        for segment in path.split('/') {
            append_slash = false;
            if segment == ".." {
                if !output.is_empty() {
                    output.pop();
                    if output.is_empty() {
                        output.push("");
                    }
                }
                append_slash = true;
            } else if segment == "." {
                append_slash = true;
            } else {
                output.push(segment);
            }
        }
        if append_slash {
            output.push("");
        }
        output.join("/")
    }

    /// `path.url.relative(to, from: dir)` over normalized
    /// `/`-separated paths: the shared segment prefix drops, every
    /// remaining `from` segment becomes `..`, the remaining `to` segments
    /// follow; an emptied result is `.`.
    fn minimal_relative(from: &str, to: &str) -> String {
        let mut from_segments: Vec<&str> = from.split('/').collect();
        if from.is_empty() {
            from_segments.clear();
        }
        let to_segments: Vec<&str> = to.split('/').collect();
        let shared = from_segments
            .iter()
            .zip(to_segments.iter())
            .take_while(|(a, b)| a == b)
            .count();
        let mut out: Vec<&str> = Vec::new();
        out.extend(from_segments[shared..].iter().map(|_| ".."));
        out.extend(&to_segments[shared..]);
        if out.is_empty() {
            return ".".to_owned();
        }
        out.join("/")
    }

    /// The `toFilePath` face: every `%HH` escape decodes to its byte.
    fn decode_percent(path: &str) -> String {
        let bytes = path.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'%'
                && let Some(hex) = bytes.get(i + 1..i + 3)
                && let Some(hi) = (hex[0] as char).to_digit(16)
                && let Some(lo) = (hex[1] as char).to_digit(16)
            {
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "two hex digits are at most 0xFF, so the pair fits u8"
                )]
                out.push((hi * 16 + lo) as u8);
                i += 3;
            } else {
                out.push(bytes[i]);
                i += 1;
            }
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    /// The file target of a directive URI: the escape-normalized,
    /// dot-segment-free, percent-decoded path (`None` when the URI
    /// carries a scheme or authority — no file face — or normalizes to
    /// nothing). An absolute candidate keeps its leading `/`.
    #[must_use]
    pub fn relative_file_path(uri: &str) -> Option<String> {
        let (scheme, authority, path, _) = split_uri(uri);
        if scheme.is_some() || authority.is_some() {
            return None;
        }
        let escaped = escape_normalize(path);
        let normalized = if escaped.starts_with('/') {
            remove_dot_segments(&escaped)
        } else {
            normalize_relative(&escaped)
        };
        // `./` is the walk's emptiness marker (`a/..`): no file target —
        // the reference resolves to the containing file, which exists.
        (!normalized.is_empty() && normalized != "./").then(|| decode_percent(&normalized))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// The upstream corpus verdicts
        /// (`test/rules/simple_directive_paths_test.dart`): step 1 — the
        /// parse round-trip catches dot segments, unnecessary escapes and
        /// backtracking paths (`test_import_relative_nonMinimal`,
        /// `test_import_package_nonMinimal`,
        /// `test_import_package_nonMinimal_backtracking`,
        /// `test_import_relative_nonMinimal_backtracking`,
        /// `test_import_absolute_backtracking`,
        /// `test_import_absolute_unnormalized`, `test_import_escape`).
        #[test]
        fn step_one_matches_the_upstream_corpus() {
            for (dir, uri) in [
                (Some("/pkg/lib"), "./a.dart"),
                (Some("/pkg/lib"), "package:test/./a.dart"),
                (Some("/pkg/lib"), "package:test/src/../a.dart"),
                (Some("/pkg/lib/src"), "../src/../a.dart"),
                (Some("/pkg/lib"), "/src/../a.dart"),
                (Some("/pkg/lib"), "/../a.dart"),
                (Some("/pkg/lib"), "/./a.dart"),
                (Some("/pkg/lib"), "%41.dart"),
                (Some("/pkg/lib"), "a%2Eb.dart"),
            ] {
                assert!(reports(dir, uri), "{uri} must report");
            }
        }

        /// Step 1 passes on already-normalized URIs regardless of the unit
        /// path (`test_import_relative_minimal`,
        /// `test_import_absolute_normalized`, `test_export_authority`,
        /// `test_import_package_minimal`) — and a scheme-less
        /// already-minimal path passes a pathless unit too (the
        /// `contextUri == null` face only gates step 2).
        #[test]
        fn step_one_passes_the_normalized_forms() {
            for (dir, uri) in [
                (Some("/pkg/lib"), "a.dart"),
                (Some("/pkg/lib"), "../a.dart"),
                (Some("/pkg/lib"), "../test/a.dart"),
                (Some("/pkg/lib"), "/a.dart"),
                (Some("/pkg/lib"), "//localhost/a.dart"),
                (Some("/pkg/lib"), "package:test/a.dart"),
                (Some("/pkg/lib"), "package:test/a.dart?x=1"),
                (Some("/pkg/lib"), "dart:math"),
                (None, "a.dart"),
            ] {
                assert!(!reports(dir, uri), "{uri} must pass");
            }
        }

        /// Step 2 — the minimal-relative face fires where the written path
        /// is normalized but not the shortest route from the unit
        /// directory (`test_import_query`, `test_import_fragment`,
        /// `test_export_relative_nonMinimal`), including the backtracking
        /// round trip through the unit's parent (`../lib/b.dart`).
        #[test]
        fn step_two_matches_the_upstream_corpus() {
            for (dir, uri) in [
                (Some("/pkg/lib"), "a.dart?key=val"),
                (Some("/pkg/lib"), "a.dart#frag"),
                (Some("/pkg/lib"), "../lib/b.dart"),
            ] {
                assert!(reports(dir, uri), "{uri} must report");
            }
        }

        /// `path.url.relative` — shared prefixes drop, the rest climbs.
        #[test]
        fn minimal_relative_walks_segments() {
            assert_eq!(minimal_relative("", "a.dart"), "a.dart");
            assert_eq!(minimal_relative("/a/b", "/a/b/c"), "c");
            assert_eq!(minimal_relative("/a/b", "/a/c"), "../c");
            assert_eq!(minimal_relative("/a/b", "/a/b"), ".");
            assert_eq!(minimal_relative("/a/b/c", "/a/x/y"), "../../x/y");
        }

        /// The escape walk: unreserved escapes decode, hex digits
        /// uppercase, a stray `%` re-encodes, disallowed characters and
        /// backslashes encode (`test_import_escape` is the `%41` face).
        #[test]
        fn escape_walk_faces() {
            assert_eq!(escape_normalize("a.dart"), "a.dart");
            assert_eq!(escape_normalize("%41%2F%4a"), "A%2FJ");
            assert_eq!(escape_normalize("%zz"), "%25zz");
            assert_eq!(escape_normalize("a b"), "a%20b");
            assert_eq!(escape_normalize("a\\b"), "a/b");
            assert_eq!(escape_normalize("caf\u{e9}.dart"), "caf%C3%A9.dart");
            assert_eq!(decode_percent("a%41b%2F"), "aAb/");
        }

        /// The two dot-segment faces: the relative walk keeps leading
        /// `..`s and surfaces an emptied path as `./`; the RFC walk
        /// clamps at the root.
        #[test]
        fn dot_segment_faces() {
            assert_eq!(normalize_relative("./a.dart"), "a.dart");
            assert_eq!(normalize_relative("a/.."), "./");
            assert_eq!(normalize_relative("../a.dart"), "../a.dart");
            assert_eq!(normalize_relative("a/../../c"), "../c");
            assert_eq!(normalize_relative("a/"), "a/");
            assert_eq!(normalize_relative(".//etc"), ".//etc");
            assert_eq!(remove_dot_segments("/a/../b.dart"), "/b.dart");
            assert_eq!(remove_dot_segments("/../a.dart"), "/a.dart");
            assert_eq!(remove_dot_segments("./a.dart"), "a.dart");
            assert_eq!(remove_dot_segments("a/.."), "/");
        }

        /// The structural split: scheme lowercased, authority verbatim,
        /// path cut at the first query/fragment delimiter.
        #[test]
        fn split_faces() {
            assert_eq!(
                split_uri("package:test/a.dart"),
                (Some("package".to_owned()), None, "test/a.dart", "")
            );
            assert_eq!(
                split_uri("PACKAGE:x/y.dart"),
                (Some("package".to_owned()), None, "x/y.dart", "")
            );
            assert_eq!(
                split_uri("//host/a.dart?q#f"),
                (None, Some("host"), "/a.dart", "?q#f")
            );
            assert_eq!(split_uri("a.dart"), (None, None, "a.dart", ""));
        }

        /// The file-target seam: relative paths normalize and decode,
        /// scheme/authority forms have no file face
        /// (`conditional_uri_does_not_exist`'s documented under-report).
        #[test]
        fn relative_file_target_faces() {
            assert_eq!(relative_file_path("a/../b.dart"), Some("b.dart".to_owned()));
            assert_eq!(relative_file_path("%41.dart"), Some("A.dart".to_owned()));
            assert_eq!(relative_file_path("/a.dart"), Some("/a.dart".to_owned()));
            assert_eq!(relative_file_path("package:x/a.dart"), None);
            assert_eq!(relative_file_path("dart:math"), None);
            assert_eq!(relative_file_path("//host/a.dart"), None);
            assert_eq!(relative_file_path("a/.."), None);
        }
    }
}

pub(crate) mod avoid_web_libraries_in_flutter {
    //! Official: `upstream Dart SDK source, pkg/linter/lib/src/rules/avoid_web_libraries_in_flutter.dart`
    //! — an import of `dart:html`, `dart:js` or `dart:js_util`
    //! (`isWebUri`, :108-114: exact cooked-`stringValue` spellings)
    //! reports at the import directive (:116-121). The registration gate
    //! is two-faced: a containing package root must exist (:78-93) and
    //! the pubspec must declare the `flutter` dependency without the web
    //! plugin platform (`hasFlutterDep`, :46-75).
    //!
    //! Approximation (documented): the library identity is the written
    //! URI spelling — exact for cooked string literals, silent on
    //! interpolation (`stringValue` null). The crate carries no
    //! flutter-context face (the pubspec's flutter/plugin shape is not a
    //! [`WorkspaceFacts`][crate::WorkspaceFacts] field), so the
    //! in-flutter predicate defaults to `true` inside any known package:
    //! `workspace: None` is the exact face of the official
    //! `package?.root == null` gate and stays silent, `Some` reports the
    //! web spellings — over-reporting in non-Flutter packages and
    //! Flutter web plugin packages, where the official stays silent, and
    //! never under-reporting the spelling itself. Message:
    //! `diagnostic.g.dart` `avoidWebLibrariesInFlutter`.

    use super::{Check, NodeKind, RuleSpec, written_uri};

    pub(crate) static SPEC: RuleSpec =
        crate::spec!("avoid_web_libraries_in_flutter", &[NodeKind::Import], check,);

    fn check(c: &mut Check<'_>) {
        // `context.package?.root` (rule_context gate): outside any known
        // package the rule never registers.
        if c.unit.workspace.is_none() {
            return;
        }
        let Some((_, uri)) = written_uri(c, c.node()) else {
            return;
        };
        if matches!(uri.as_str(), "dart:html" | "dart:js" | "dart:js_util") {
            // The official reports at the directive node, not the URI.
            c.report(c.utf16_span(c.node()), &[]);
        }
    }
}

pub(crate) mod conditional_uri_does_not_exist {
    //! Official: `upstream Dart SDK source, pkg/linter/lib/src/rules/conditional_uri_does_not_exist.dart`
    //! — a configured URI (`if (condition) 'uri'`) whose resolution has no
    //! existing source reports at the candidate URI with the relative URI
    //! string in the message (:37-49). The dartq parser retains the
    //! clause on the lint face: the bridge's `begin_conditional_uri`
    //! opens the [`NodeKind::ConditionalUri`] at the `if` and the
    //! clause's string events land as its direct children, so the
    //! candidate is the node's last string literal (an `==` comparison
    //! value, when present, precedes it) — the shape the
    //! `avoid_relative_lib_imports` fixture pins.
    //!
    //! Approximation (documented): the official existence check resolves
    //! every URI form through the SDK library table and the package
    //! config (the missing `dart:`/`package:` faces of the upstream
    //! corpus, `test_missingDartLibraries`/`test_missingPackages`); this
    //! face carries neither table, so the relative-URI form is the ported
    //! one — resolve against the analyzed file's directory (the
    //! `contextUri` face, the unit's own path for parts) and test the
    //! file system. Scheme'd and authority'd candidates stay silent:
    //! unresolvable-library forms under-report, a resolvable target is
    //! silent both ways. The recovery-spliced empty candidate and an
    //! interpolated one never report. A pathless unit stays silent (no
    //! directory to resolve against). Message: `diagnostic.g.dart`
    //! `conditionalUriDoesNotExist`.

    use std::path::PathBuf;

    use super::{Check, NodeKind, RuleSpec, candidate_uri};

    pub(crate) static SPEC: RuleSpec = crate::spec!(
        "conditional_uri_does_not_exist",
        &[NodeKind::ConditionalUri],
        check,
    );

    fn check(c: &mut Check<'_>) {
        let Some((range, candidate)) = candidate_uri(c, c.node()) else {
            return;
        };
        // An empty candidate resolves to the importing file itself, which
        // exists (the recovery splice and the written `''` form alike).
        if candidate.is_empty() {
            return;
        }
        let Some(path) = super::simple_directive_paths::relative_file_path(&candidate) else {
            return;
        };
        let Some(unit_path) = c.unit.path else {
            return;
        };
        let target = if path.starts_with('/') {
            PathBuf::from(path)
        } else {
            match unit_path.parent() {
                Some(dir) => dir.join(path),
                None => return,
            }
        };
        // The official `source?.exists()` — the same follow-symlinks stat.
        if !target.exists() {
            c.report(range, &[candidate.as_str()]);
        }
    }
}

/// The #434 dispatch pins: `depend_on_referenced_packages` and
/// `implementation_imports` over the real parser trees, with the
/// containing-package facts mounted (`Unit.workspace`).
#[cfg(test)]
mod workspace_pins {
    use crate::WorkspaceFacts;
    use crate::registry::rule_by_name;
    use crate::testsupport::{Fixture, one_rule};
    use rustc_hash::FxHashSet;
    use std::path::{Path, PathBuf};

    /// Facts for a package at `root` named `own`, dependencies `deps`,
    /// dev dependencies `dev`.
    fn facts(own: &str, deps: &[&str], dev: &[&str], root: &Path) -> WorkspaceFacts {
        let collect = |names: &[&str]| -> FxHashSet<String> {
            names.iter().map(|name| (*name).to_owned()).collect()
        };
        let mut dependencies = collect(deps);
        dependencies.insert(own.to_owned());
        WorkspaceFacts {
            own_package: own.to_owned(),
            dependencies,
            dev_dependencies: collect(dev),
            root: root.to_owned(),
            lib_dir: root.join("lib"),
        }
    }

    /// Hits as `(start, end, message)`, run with `facts` mounted and the
    /// fixture's analyzed path set to `path`.
    fn hits(
        f: &mut Fixture,
        path: &str,
        rule: &str,
        facts: &WorkspaceFacts,
    ) -> Vec<(u32, u32, String)> {
        f.path = PathBuf::from(path);
        let id = rule_by_name(rule).expect("registered rule");
        f.run_workspace(&one_rule(rule), facts)
            .into_iter()
            .map(|l| {
                assert_eq!(l.rule, id);
                (l.range.start, l.range.end, l.message)
            })
            .collect()
    }

    const ROOT: &str = "/pkg";

    /// A foreign package's import reports at the URI span with the
    /// official argument fill; a listed dependency and the own package
    /// stay silent.
    #[test]
    fn foreign_import_reports_at_the_uri() {
        let facts = facts("own", &["meta"], &[], Path::new(ROOT));
        let mut f = Fixture::parse("import 'package:foreign/foreign.dart';\n");
        assert_eq!(
            hits(
                &mut f,
                "/pkg/lib/a.dart",
                "depend_on_referenced_packages",
                &facts
            ),
            vec![(
                7,
                37,
                "The imported package 'foreign' isn't a dependency of the importing package."
                    .to_owned()
            )]
        );
        let mut dep = Fixture::parse("import 'package:meta/meta.dart';\n");
        assert!(
            hits(
                &mut dep,
                "/pkg/lib/a.dart",
                "depend_on_referenced_packages",
                &facts
            )
            .is_empty()
        );
        let mut own = Fixture::parse("import 'package:own/src/inner.dart';\n");
        assert!(
            hits(
                &mut own,
                "/pkg/lib/a.dart",
                "depend_on_referenced_packages",
                &facts
            )
            .is_empty()
        );
    }

    /// Dev dependencies are available only outside the public dirs
    /// (isInPublicDir, ast.dart:112-129): a `lib/` file reports, a
    /// `test/` file stays silent.
    #[test]
    fn dev_dependencies_gate_on_the_public_dir() {
        let facts = facts("own", &[], &["dev_pkg"], Path::new(ROOT));
        let mut f = Fixture::parse("import 'package:dev_pkg/dev_pkg.dart';\n");
        assert_eq!(
            hits(
                &mut f,
                "/pkg/lib/a.dart",
                "depend_on_referenced_packages",
                &facts
            )
            .len(),
            1,
            "lib/ is public: the dev dependency is unavailable"
        );
        assert!(
            hits(
                &mut f,
                "/pkg/test/a_test.dart",
                "depend_on_referenced_packages",
                &facts
            )
            .is_empty(),
            "test/ is private: the dev dependency is available"
        );
    }

    /// Exports dispatch like imports (:59-60).
    #[test]
    fn exports_dispatch() {
        let facts = facts("own", &[], &[], Path::new(ROOT));
        let mut f = Fixture::parse("export 'package:foreign/foreign.dart';\n");
        assert_eq!(
            hits(
                &mut f,
                "/pkg/lib/a.dart",
                "depend_on_referenced_packages",
                &facts
            )
            .len(),
            1
        );
    }

    /// A foreign `src/` import from inside the own `lib/` reports at the
    /// URI; the same package's `src/` stays silent; outside `lib/` (no
    /// `package:` library URI) the rule never runs.
    #[test]
    fn implementation_import_gates() {
        let facts = facts("own", &[], &[], Path::new(ROOT));
        let mut f = Fixture::parse("import 'package:foo/src/foo.dart';\n");
        assert_eq!(
            hits(&mut f, "/pkg/lib/a.dart", "implementation_imports", &facts),
            vec![(
                7,
                33,
                "Import of a library in the 'lib/src' directory of another package.".to_owned()
            )]
        );
        let mut own = Fixture::parse("import 'package:own/src/inner.dart';\n");
        assert!(
            hits(
                &mut own,
                "/pkg/lib/a.dart",
                "implementation_imports",
                &facts
            )
            .is_empty()
        );
        assert!(
            hits(
                &mut f,
                "/pkg/test/a_test.dart",
                "implementation_imports",
                &facts
            )
            .is_empty(),
            "outside the package-uri dir the library URI is not a package: URI"
        );
        let mut relative = Fixture::parse("import '../other/lib/src/a.dart';\n");
        assert!(
            hits(
                &mut relative,
                "/pkg/lib/a.dart",
                "implementation_imports",
                &facts
            )
            .is_empty(),
            "a relative URI resolves to a file: URI, never an implementation import"
        );
    }

    /// No containing package (`workspace: None`) keeps both rules silent —
    /// the official no-pubspec registration gate.
    #[test]
    fn no_package_face_stays_silent() {
        let f = Fixture::parse("import 'package:foreign/src/a.dart';\n");
        for rule in ["depend_on_referenced_packages", "implementation_imports"] {
            assert!(f.run(&one_rule(rule)).is_empty(), "{rule}");
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::engine::Features;
    use crate::registry::rule_by_name;
    use crate::testsupport::{Fixture, NONE, identifier, one_rule, tok, unit_of};
    use dartq_ast::NodeKind;

    /// One reported hit as `(start, end, message)`.
    fn hits(f: &Fixture, rule: &str) -> Vec<(u32, u32, String)> {
        let id = rule_by_name(rule).expect("registered rule");
        f.run(&one_rule(rule))
            .into_iter()
            .map(|l| {
                assert_eq!(l.rule, id);
                (l.range.start, l.range.end, l.message)
            })
            .collect()
    }

    /// An `Import` over `import <str> as <prefix>;` (when `as_kw`) or
    /// `import <str>;`: the `as` keyword back-fills payload word 0
    /// (the `ImportPrefixHandle` slot), the prefix is the
    /// Identifier child.
    fn import_fixture(source: &str, as_kw: bool) -> Fixture {
        Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::Import, 0);
                b.patch(0, u32::from(as_kw) * 2);
                b.patch(1, NONE);
                b.leaf_span(NodeKind::StringLiteral, 1, 2);
                if as_kw {
                    identifier(b, 3, 4);
                }
                b.end(NodeKind::Import, if as_kw { 5 } else { 3 });
            }),
        )
    }

    /// The `library_annotations` walk reads a `MetadataStar` container under
    /// the unit whose annotated sibling follows it. This builds
    /// `@pragma(<str>)` + `var x = 1;` — a `var` declaration carries no
    /// directive prelude, so the annotated sibling is the fields node
    /// itself; wrapper shapes are covered by the export/library tests.
    /// `extra_arg` adds a second positional argument after the string.
    fn var_pragma_fixture(extra_arg: bool) -> Fixture {
        let (source, head, end) = if extra_arg {
            ("@pragma('dart2js:late:trust', extra)\nvar x = 1;", 7, 12)
        } else {
            ("@pragma('dart2js:late:trust')\nvar x = 1;", 5, 10)
        };
        Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::MetadataStar, 0);
                b.begin(NodeKind::Metadata, 1);
                b.patch(0, NONE);
                identifier(b, 1, 2);
                b.begin(NodeKind::Arguments, 2);
                b.leaf_span(NodeKind::StringLiteral, 3, 4);
                if extra_arg {
                    identifier(b, 5, 6);
                }
                b.end(NodeKind::Arguments, if extra_arg { 7 } else { 5 });
                b.end(NodeKind::Metadata, if extra_arg { 7 } else { 5 });
                b.end(NodeKind::MetadataStar, if extra_arg { 7 } else { 5 });
                b.begin(NodeKind::TopLevelFields, head);
                b.begin(NodeKind::VariablesDeclaration, head);
                b.patch(0, NONE);
                b.end(NodeKind::VariablesDeclaration, end);
                b.end(NodeKind::TopLevelFields, end);
            }),
        )
    }

    // —— avoid_relative_lib_imports ———————————————————————————————————————

    /// `test_samePackage_relativeUri` — reports at the URI.
    #[test]
    fn relative_lib_import_reports_at_uri() {
        let f = Fixture::build(
            "import '../lib/lib.dart';",
            unit_of(|b, _t| {
                b.begin(NodeKind::Import, 0);
                b.patch(0, NONE);
                b.patch(1, NONE);
                b.leaf_span(NodeKind::StringLiteral, 1, 2);
                b.end(NodeKind::Import, 3);
            }),
        );
        assert_eq!(hits(&f, "avoid_relative_lib_imports").len(), 1);
    }

    /// Scheme forms and a `/lib/` without a preceding slash do not hit
    /// (`isRelativeLibImport`: scheme empty + `path.contains('/lib/')`,
    /// :24-35).
    #[test]
    fn relative_lib_import_scheme_and_lead_slash_shapes() {
        for (source, expected) in [
            ("import 'foo/lib/a.dart';", 1),
            ("import 'lib/a.dart';", 0),
            ("import 'package:x/lib.dart';", 0),
            ("import 'http://x/lib/a.dart';", 0),
            ("import 'a.dart';", 0),
        ] {
            let f = Fixture::build(
                source,
                unit_of(|b, _t| {
                    b.begin(NodeKind::Import, 0);
                    b.patch(0, NONE);
                    b.patch(1, NONE);
                    b.leaf_span(NodeKind::StringLiteral, 1, 2);
                    b.end(NodeKind::Import, 3);
                }),
            );
            assert_eq!(
                hits(&f, "avoid_relative_lib_imports").len(),
                expected,
                "{source}"
            );
        }
    }

    /// Only the import URI is checked — conditional-URI strings are separate
    /// nodes (`node.uri`, :44-48).
    #[test]
    fn relative_lib_import_checks_only_the_uri() {
        let f = Fixture::build(
            "import 'a.dart' if (x) '../lib/b.dart';",
            unit_of(|b, _t| {
                b.begin(NodeKind::Import, 0);
                b.patch(0, NONE);
                b.patch(1, NONE);
                b.leaf_span(NodeKind::StringLiteral, 1, 2);
                b.begin(NodeKind::ConditionalUri, 2);
                b.patch(0, 3);
                b.patch(1, NONE);
                b.leaf_span(NodeKind::StringLiteral, 6, 7);
                b.end(NodeKind::ConditionalUri, 8);
                b.end(NodeKind::Import, 8);
            }),
        );
        assert!(hits(&f, "avoid_relative_lib_imports").is_empty());
    }

    /// An interpolated URI has no `stringValue` (:27) — the `StringLiteral`
    /// spans every string-part token.
    #[test]
    fn relative_lib_import_interpolated_uri_is_inert() {
        let source = "import '$x/lib/a.dart';";
        let f = Fixture::build(
            source,
            unit_of(|b, t| {
                let semi = tok(source, t, ";", 0);
                b.begin(NodeKind::Import, 0);
                b.patch(0, NONE);
                b.patch(1, NONE);
                b.begin(NodeKind::StringLiteral, 1);
                b.end(NodeKind::StringLiteral, semi);
                b.end(NodeKind::Import, semi + 1);
            }),
        );
        assert!(hits(&f, "avoid_relative_lib_imports").is_empty());
    }

    // —— library_annotations —————————————————————————————————————————————

    /// `test_topLevelVariableDeclaration` — the pragma on a top-level
    /// variable reports at the annotation (no directive prelude wrapper on
    /// the `var` declaration).
    #[test]
    fn pragma_on_top_level_variable_reports() {
        let f = var_pragma_fixture(false);
        assert_eq!(hits(&f, "library_annotations").len(), 1);
    }

    /// `test_exportDeclaration` — on a non-library directive.
    #[test]
    fn pragma_on_export_reports() {
        let f = Fixture::build(
            "@pragma('dart2js:late:trust')\nexport 'dart:math';",
            unit_of(|b, _t| {
                b.begin(NodeKind::MetadataStar, 0);
                b.begin(NodeKind::Metadata, 1);
                b.patch(0, NONE);
                identifier(b, 1, 2);
                b.begin(NodeKind::Arguments, 2);
                b.leaf_span(NodeKind::StringLiteral, 3, 4);
                b.end(NodeKind::Arguments, 5);
                b.end(NodeKind::Metadata, 5);
                b.end(NodeKind::MetadataStar, 5);
                b.begin(NodeKind::TopLevelDeclaration, 5);
                b.begin(NodeKind::Export, 5);
                b.leaf_span(NodeKind::StringLiteral, 6, 7);
                b.end(NodeKind::Export, 8);
                b.end(NodeKind::TopLevelDeclaration, 8);
            }),
        );
        assert_eq!(hits(&f, "library_annotations").len(), 1);
    }

    /// `test_libraryDirective` — the `library` directive is never checked
    /// (:54-55).
    #[test]
    fn pragma_on_library_directive_is_inert() {
        let f = Fixture::build(
            "@pragma('dart2js:late:trust')\nlibrary foo;",
            unit_of(|b, _t| {
                b.begin(NodeKind::MetadataStar, 0);
                b.begin(NodeKind::Metadata, 1);
                b.patch(0, NONE);
                identifier(b, 1, 2);
                b.begin(NodeKind::Arguments, 2);
                b.leaf_span(NodeKind::StringLiteral, 3, 4);
                b.end(NodeKind::Arguments, 5);
                b.end(NodeKind::Metadata, 5);
                b.end(NodeKind::MetadataStar, 5);
                b.begin(NodeKind::TopLevelDeclaration, 5);
                b.begin(NodeKind::LibraryName, 5);
                b.patch(0, 1);
                identifier(b, 6, 7);
                b.end(NodeKind::LibraryName, 8);
                b.end(NodeKind::TopLevelDeclaration, 8);
            }),
        );
        assert!(hits(&f, "library_annotations").is_empty());
    }

    /// `test_partOfFile` — a `part of` unit ends the walk (:51-53).
    #[test]
    fn part_of_unit_is_inert() {
        let f = Fixture::build(
            "@pragma('dart2js:late:trust')\npart of 'a.dart';",
            unit_of(|b, _t| {
                b.begin(NodeKind::MetadataStar, 0);
                b.begin(NodeKind::Metadata, 1);
                b.patch(0, NONE);
                identifier(b, 1, 2);
                b.begin(NodeKind::Arguments, 2);
                b.leaf_span(NodeKind::StringLiteral, 3, 4);
                b.end(NodeKind::Arguments, 5);
                b.end(NodeKind::Metadata, 5);
                b.end(NodeKind::MetadataStar, 5);
                b.begin(NodeKind::TopLevelDeclaration, 5);
                b.begin(NodeKind::PartOf, 5);
                b.patch(0, 6);
                b.patch(1, 0);
                b.leaf_span(NodeKind::StringLiteral, 7, 8);
                b.end(NodeKind::PartOf, 9);
                b.end(NodeKind::TopLevelDeclaration, 9);
            }),
        );
        assert!(hits(&f, "library_annotations").is_empty());
    }

    /// A second positional argument is a compile error upstream (the pragma
    /// factory takes one positional), so the constant never resolves and the
    /// branch never fires (:84-95).
    #[test]
    fn two_argument_pragma_is_inert() {
        let f = var_pragma_fixture(true);
        assert!(hits(&f, "library_annotations").is_empty());
    }

    /// Annotations other than the pinned pragma are inert at this face —
    /// `@foo` carries no `pragma` name.
    #[test]
    fn other_annotation_is_inert() {
        let f = Fixture::build(
            "@foo\nvar x = 1;",
            unit_of(|b, _t| {
                b.begin(NodeKind::MetadataStar, 0);
                b.begin(NodeKind::Metadata, 1);
                b.patch(0, NONE);
                identifier(b, 1, 2);
                b.end(NodeKind::Metadata, 2);
                b.end(NodeKind::MetadataStar, 2);
                b.begin(NodeKind::TopLevelFields, 2);
                b.begin(NodeKind::VariablesDeclaration, 2);
                b.patch(0, NONE);
                b.end(NodeKind::VariablesDeclaration, 6);
                b.end(NodeKind::TopLevelFields, 6);
            }),
        );
        assert!(hits(&f, "library_annotations").is_empty());
    }

    /// A pragma value other than the pinned string is inert (:92).
    #[test]
    fn wrong_pragma_value_is_inert() {
        let f = Fixture::build(
            "@pragma('other')\nvar x = 1;",
            unit_of(|b, _t| {
                b.begin(NodeKind::MetadataStar, 0);
                b.begin(NodeKind::Metadata, 1);
                b.patch(0, NONE);
                identifier(b, 1, 2);
                b.begin(NodeKind::Arguments, 2);
                b.leaf_span(NodeKind::StringLiteral, 3, 4);
                b.end(NodeKind::Arguments, 5);
                b.end(NodeKind::Metadata, 5);
                b.end(NodeKind::MetadataStar, 5);
                b.begin(NodeKind::TopLevelFields, 5);
                b.begin(NodeKind::VariablesDeclaration, 5);
                b.patch(0, NONE);
                b.end(NodeKind::VariablesDeclaration, 10);
                b.end(NodeKind::TopLevelFields, 10);
            }),
        );
        assert!(hits(&f, "library_annotations").is_empty());
    }

    /// Member metadata is never walked (:48-58 — unit directives and unit
    /// declarations only).
    #[test]
    fn member_metadata_is_inert() {
        let f = Fixture::build(
            "class C {\n  @pragma('dart2js:late:trust')\n  var x;\n}",
            unit_of(|b, _t| {
                b.begin(NodeKind::Fields, 0);
                b.patch(0, 0); // MemberKind (the value is irrelevant here)
                b.begin(NodeKind::MetadataStar, 3);
                b.begin(NodeKind::Metadata, 4);
                b.patch(0, NONE);
                identifier(b, 4, 5);
                b.begin(NodeKind::Arguments, 5);
                b.leaf_span(NodeKind::StringLiteral, 6, 7);
                b.end(NodeKind::Arguments, 8);
                b.end(NodeKind::Metadata, 8);
                b.end(NodeKind::MetadataStar, 8);
                b.end(NodeKind::Fields, 12);
            }),
        );
        assert!(hits(&f, "library_annotations").is_empty());
    }

    // —— library_prefixes / no_leading_underscores_for_library_prefixes ———

    /// `test_camelCase` reports at the prefix; `$async` passes
    /// (`utils.dart:58` optional `$`), `_1` fails (no lowercase after the
    /// underscores).
    #[test]
    fn library_prefixes_styles() {
        let bad = import_fixture("import 'a.dart' as dartAsync;", true);
        let h = hits(&bad, "library_prefixes");
        assert_eq!(h.len(), 1);
        assert_eq!(
            h[0].2,
            "The prefix 'dartAsync' isn't a lower_case_with_underscores identifier."
        );

        for good in ["import 'a.dart' as _async;", "import 'a.dart' as $async;"] {
            let f = import_fixture(good, true);
            assert!(hits(&f, "library_prefixes").is_empty(), "{good}");
        }
        let underscored = import_fixture("import 'a.dart' as _1;", true);
        assert_eq!(hits(&underscored, "library_prefixes").len(), 1);
    }

    /// `test_wildcard` — a lone `_` prefix passes with `wildcard_variables`
    /// and reports without it (:36-39, `test_wildcard_preWildCards`).
    #[test]
    fn wildcard_prefix_feature_gate() {
        let f = import_fixture("import 'a.dart' as _;", true);
        assert!(hits(&f, "library_prefixes").is_empty());
        assert!(hits(&f, "no_leading_underscores_for_library_prefixes").is_empty());
        let mut f = import_fixture("import 'a.dart' as _;", true);
        f.features = Features {
            wildcard_variables: false,
            ..Features::default()
        };
        assert_eq!(hits(&f, "library_prefixes").len(), 1);
        assert_eq!(
            hits(&f, "no_leading_underscores_for_library_prefixes").len(),
            1
        );
    }

    /// A leading-underscore prefix reports at the prefix; a plain one does
    /// not.
    #[test]
    fn underscored_library_prefix_reports() {
        let f = import_fixture("import 'a.dart' as _async;", true);
        let h = hits(&f, "no_leading_underscores_for_library_prefixes");
        assert_eq!(h.len(), 1);
        assert_eq!(
            h[0].2,
            "The library prefix '_async' starts with an underscore."
        );
        let plain = import_fixture("import 'a.dart' as async;", true);
        assert!(hits(&plain, "no_leading_underscores_for_library_prefixes").is_empty());
    }

    // —— unnecessary_library_name ————————————————————————————————————————

    /// A named `library` directive reports at the name; the bare `library;`
    /// form (`has_name` false) and the feature-off gate do not.
    #[test]
    fn library_name_forms() {
        let named = |features: bool| {
            let mut f = Fixture::build(
                "library foo;",
                unit_of(|b, _t| {
                    b.begin(NodeKind::LibraryName, 0);
                    b.patch(0, 1);
                    identifier(b, 1, 2);
                    b.end(NodeKind::LibraryName, 3);
                }),
            );
            if !features {
                f.features = Features {
                    unnamed_libraries: false,
                    ..Features::default()
                };
            }
            f
        };
        let h = hits(&named(true), "unnecessary_library_name");
        assert_eq!(h.len(), 1);
        assert_eq!(h[0].2, "Library names are not necessary.");
        assert!(hits(&named(false), "unnecessary_library_name").is_empty());

        let bare = Fixture::build(
            "library;",
            unit_of(|b, _t| {
                b.begin(NodeKind::LibraryName, 0);
                b.patch(0, 0);
                b.end(NodeKind::LibraryName, 2);
            }),
        );
        assert!(hits(&bare, "unnecessary_library_name").is_empty());
    }

    /// A dotted name reports across the whole qualified span (the name child
    /// is the qualified node; nesting follows `handleQualified`).
    #[test]
    fn library_name_dotted_reports() {
        let f = Fixture::build(
            "library a.b.c;",
            unit_of(|b, _t| {
                b.begin(NodeKind::LibraryName, 0);
                b.patch(0, 1);
                b.begin(NodeKind::Qualified, 1);
                b.patch(0, 2);
                identifier(b, 1, 2);
                b.begin(NodeKind::Qualified, 3);
                b.patch(0, 4);
                identifier(b, 3, 4);
                identifier(b, 5, 6);
                b.end(NodeKind::Qualified, 7);
                b.end(NodeKind::Qualified, 7);
                b.end(NodeKind::LibraryName, 7);
            }),
        );
        assert_eq!(hits(&f, "unnecessary_library_name").len(), 1);
    }

    // —— use_string_in_part_of_directives ————————————————————————————————

    /// `part of foo;` reports at the directive; the URI form, a plain
    /// `part`, and the `enhanced_parts` gate do not.
    #[test]
    fn part_of_forms() {
        let named = |enhanced: bool| {
            let mut f = Fixture::build(
                "part of foo;",
                unit_of(|b, _t| {
                    b.begin(NodeKind::PartOf, 0);
                    b.patch(0, 1);
                    b.patch(1, 1);
                    identifier(b, 2, 3);
                    b.end(NodeKind::PartOf, 4);
                }),
            );
            if enhanced {
                f.features = Features {
                    enhanced_parts: true,
                    ..Features::default()
                };
            }
            f
        };
        let h = hits(&named(false), "use_string_in_part_of_directives");
        assert_eq!(h.len(), 1);
        assert_eq!(h[0].2, "The part-of directive uses a library name.");
        assert!(hits(&named(true), "use_string_in_part_of_directives").is_empty());

        let uri = Fixture::build(
            "part of 'a.dart';",
            unit_of(|b, _t| {
                b.begin(NodeKind::PartOf, 0);
                b.patch(0, 1);
                b.patch(1, 0);
                b.leaf_span(NodeKind::StringLiteral, 2, 3);
                b.end(NodeKind::PartOf, 4);
            }),
        );
        assert!(hits(&uri, "use_string_in_part_of_directives").is_empty());

        let part = Fixture::build(
            "part 'a.dart';",
            unit_of(|b, _t| {
                b.begin(NodeKind::Part, 0);
                b.leaf_span(NodeKind::StringLiteral, 1, 2);
                b.end(NodeKind::Part, 3);
            }),
        );
        assert!(hits(&part, "use_string_in_part_of_directives").is_empty());
    }

    // —— provide_deprecation_message ———————————————————————————————————————

    /// `test/rules/provide_deprecation_message_test.dart` — a bare
    /// `@deprecated` reports at the annotation.
    #[test]
    fn bare_deprecated_reports() {
        let source = "@deprecated var x = 1;";
        // @0 deprecated1 var2 x3 =4 15 ;6
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::Metadata, 0);
                b.patch(0, NONE);
                identifier(b, 1, 2);
                b.end(NodeKind::Metadata, 2);
                b.begin(NodeKind::VariablesDeclaration, 2);
                b.patch(0, NONE);
                b.begin(NodeKind::InitializedIdentifier, 3);
                identifier(b, 3, 4);
                b.begin(NodeKind::VariableInitializer, 4);
                b.leaf(NodeKind::LiteralInt, 5);
                b.end(NodeKind::VariableInitializer, 6);
                b.end(NodeKind::InitializedIdentifier, 6);
                b.end(NodeKind::VariablesDeclaration, 7);
            }),
        );
        let lints = f.run(&one_rule("provide_deprecation_message"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].message, "Missing a deprecation message.");
        assert_eq!(
            lints[0].correction,
            Some(
                "Try using the constructor to provide a message ('@Deprecated(\"message\")')."
                    .to_owned()
            )
        );
    }

    /// An argument list (even empty) keeps the message slot; the `Deprecated`
    /// constructor already carries one.
    #[test]
    fn deprecated_with_arguments_passes() {
        let source = "@deprecated() var x = 1;";
        // @0 deprecated1 (2 )3 var4 x5 =6 17 ;8
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::Metadata, 0);
                b.patch(0, NONE);
                identifier(b, 1, 2);
                b.begin(NodeKind::Arguments, 2);
                b.end(NodeKind::Arguments, 4);
                b.end(NodeKind::Metadata, 4);
                b.begin(NodeKind::VariablesDeclaration, 4);
                b.patch(0, NONE);
                b.begin(NodeKind::InitializedIdentifier, 5);
                identifier(b, 5, 6);
                b.begin(NodeKind::VariableInitializer, 6);
                b.leaf(NodeKind::LiteralInt, 7);
                b.end(NodeKind::VariableInitializer, 8);
                b.end(NodeKind::InitializedIdentifier, 8);
                b.end(NodeKind::VariablesDeclaration, 9);
            }),
        );
        assert!(f.run(&one_rule("provide_deprecation_message")).is_empty());

        let constructor = "@Deprecated('m') var x = 1;";
        // @0 Deprecated1 (2 'm'3 )4 var5 ...
        let f = Fixture::build(
            constructor,
            unit_of(|b, _t| {
                b.begin(NodeKind::Metadata, 0);
                b.patch(0, NONE);
                identifier(b, 1, 2);
                b.begin(NodeKind::Arguments, 2);
                b.leaf_span(NodeKind::StringLiteral, 3, 4);
                b.end(NodeKind::Arguments, 5);
                b.end(NodeKind::Metadata, 5);
                b.begin(NodeKind::VariablesDeclaration, 5);
                b.patch(0, NONE);
                b.begin(NodeKind::InitializedIdentifier, 6);
                identifier(b, 6, 7);
                b.begin(NodeKind::VariableInitializer, 7);
                b.leaf(NodeKind::LiteralInt, 8);
                b.end(NodeKind::VariableInitializer, 9);
                b.end(NodeKind::InitializedIdentifier, 9);
                b.end(NodeKind::VariablesDeclaration, 10);
            }),
        );
        assert!(f.run(&one_rule("provide_deprecation_message")).is_empty());
    }

    // —— element-model pins (#468, production-bridge fixtures) ————————————

    /// The pragma face reports on a non-library target; a prefixed
    /// `foo.pragma` matches only when a same-file import binds its prefix
    /// to dart:core (the `libraryName: 'dart.core'` pin,
    /// library_annotations.dart:84-95), honoring the import's combinators.
    #[test]
    fn library_annotations_prefixed_pragma_is_inert() {
        let f = Fixture::parse("@pragma('dart2js:late:trust') class C {}");
        assert_eq!(hits(&f, "library_annotations").len(), 1);

        let f = Fixture::parse("@foo.pragma('dart2js:late:trust') class C {}");
        assert!(hits(&f, "library_annotations").is_empty());

        let f = Fixture::parse(
            "import 'dart:core' as foo;\n@foo.pragma('dart2js:late:trust') class C {}",
        );
        assert_eq!(hits(&f, "library_annotations").len(), 1);

        let f = Fixture::parse(
            "import 'dart:math' as foo;\n@foo.pragma('dart2js:late:trust') class C {}",
        );
        assert!(hits(&f, "library_annotations").is_empty());

        let f = Fixture::parse(
            "import 'dart:core' as foo hide pragma;\n@foo.pragma('dart2js:late:trust') class C {}",
        );
        assert!(hits(&f, "library_annotations").is_empty());

        let f = Fixture::parse(
            "import 'dart:core' as foo show min;\n@foo.pragma('dart2js:late:trust') class C {}",
        );
        assert!(hits(&f, "library_annotations").is_empty());

        let f = Fixture::parse(
            "import 'dart:core' as foo show pragma;\n@foo.pragma('dart2js:late:trust') class C {}",
        );
        assert_eq!(hits(&f, "library_annotations").len(), 1);
    }

    /// The targetKinds branch (:61-66) over the same-file written face: an
    /// annotation declaration whose `@Target` pins exactly
    /// `TargetKind.library` reports on the unit's first directive
    /// (`test_importDirective_annotationWithTargetKindLibrary`) and passes
    /// on a declaration
    /// (`test_functionDeclaration_annotationWithTargetKindLibrary`); a
    /// multi-kind `@Target` never matches.
    #[test]
    fn library_annotations_same_file_target_library_face() {
        let f = Fixture::parse(
            "@TestOn('browser')\nimport 'dart:math';\n\n@Target({TargetKind.library})\nclass TestOn {\n  const TestOn(String name);\n}\n\nclass C {}\n",
        );
        assert_eq!(hits(&f, "library_annotations").len(), 1);

        let f = Fixture::parse(
            "@Target({TargetKind.library})\nclass TestOn {\n  const TestOn(String name);\n}\n\n@TestOn('browser')\nclass C {}\n",
        );
        assert!(hits(&f, "library_annotations").is_empty());

        let f = Fixture::parse(
            "@Target({TargetKind.library, TargetKind.class_})\nclass TestOn {\n  const TestOn(String name);\n}\n\n@TestOn('browser')\nimport 'dart:math';\n",
        );
        assert!(hits(&f, "library_annotations").is_empty());
    }

    /// A same-file `pragma` declaration shadows the dart:core constructor —
    /// the element identity (:84-95) bails; a wrong pragma value is inert.
    #[test]
    fn library_annotations_same_file_shadow_and_value_gate() {
        let f = Fixture::parse("class pragma {}\n@pragma('dart2js:late:trust') class C {}");
        assert!(hits(&f, "library_annotations").is_empty());

        let f = Fixture::parse("@pragma('other') class C {}");
        assert!(hits(&f, "library_annotations").is_empty());
    }

    /// A bare `@deprecated` (the dart:core constant, no argument list)
    /// reports; a prefixed `foo.deprecated` matches only through a
    /// same-file dart:core import; `@Deprecated` — the class, not the
    /// constant (:37-42) — and an argument-carrying form pass; a same-file
    /// `deprecated` declaration bails.
    #[test]
    fn provide_deprecation_message_constant_faces() {
        let f = Fixture::parse("@deprecated int top = 0;");
        assert_eq!(hits(&f, "provide_deprecation_message").len(), 1);

        let f = Fixture::parse("@foo.deprecated int top = 0;");
        assert!(hits(&f, "provide_deprecation_message").is_empty());

        let f = Fixture::parse("import 'dart:core' as foo;\n@foo.deprecated int top = 0;");
        assert_eq!(hits(&f, "provide_deprecation_message").len(), 1);

        let f = Fixture::parse(
            "import 'dart:core' as foo show deprecated;\n@foo.deprecated int top = 0;",
        );
        assert_eq!(hits(&f, "provide_deprecation_message").len(), 1);

        let f = Fixture::parse(
            "import 'dart:core' as foo hide deprecated;\n@foo.deprecated int top = 0;",
        );
        assert!(hits(&f, "provide_deprecation_message").is_empty());

        let f = Fixture::parse("@Deprecated int top = 0;");
        assert!(hits(&f, "provide_deprecation_message").is_empty());

        let f = Fixture::parse("@deprecated() int top = 0;");
        assert!(hits(&f, "provide_deprecation_message").is_empty());

        let f = Fixture::parse("const String deprecated = 'x';\n@deprecated int top = 0;");
        assert!(hits(&f, "provide_deprecation_message").is_empty());
    }
}

/// The W3 (#992) production-bridge pins: `combinators_ordering` and
/// `migrate_design_widgets` over real parser trees.
#[cfg(test)]
mod w3_pins {
    use crate::testsupport::{Fixture, one_rule, tok, utf16_at};

    // —— combinators_ordering —————————————————————————————————————————

    /// Unsorted show/hide combinators report at the combinator
    /// (`combinators_ordering.dart` doc examples).
    #[test]
    fn combinators_ordering_unsorted_reports() {
        let f = Fixture::parse(
            "import 'dart:math' show min, max;\nexport 'dart:math' hide zeta, alpha;\n",
        );
        let lints = f.run(&one_rule("combinators_ordering"));
        assert_eq!(lints.len(), 2, "{lints:?}");
        assert_eq!(lints[0].message, "Sort combinator names alphabetically.");
        let show = tok(&f.source, &f.tokens, "show", 0);
        assert_eq!(lints[0].range.start, utf16_at(&f, show));
        let hide = tok(&f.source, &f.tokens, "hide", 0);
        assert_eq!(lints[1].range.start, utf16_at(&f, hide));
    }

    /// Sorted combinators pass — case-sensitive code-unit order, so `B`
    /// sorts before `a` — and a single name never violates the order.
    #[test]
    fn combinators_ordering_sorted_passes() {
        let f = Fixture::parse(
            "import 'dart:async' show Completer, Stream;\nimport 'dart:convert' hide Base64, ascii;\n",
        );
        assert!(f.run(&one_rule("combinators_ordering")).is_empty());

        let f = Fixture::parse("import 'dart:math' show max;\n");
        assert!(f.run(&one_rule("combinators_ordering")).is_empty());
    }

    // —— migrate_design_widgets —————————————————————————————————————————

    /// The deprecated design library imports report at the URI with the
    /// value spelled in the message (`migrate_design_widgets.dart` doc
    /// example).
    #[test]
    fn migrate_design_widgets_reports_the_design_imports() {
        let f = Fixture::parse(
            "import 'package:flutter/material.dart';\nimport 'package:flutter/cupertino.dart';\n",
        );
        let lints = f.run(&one_rule("migrate_design_widgets"));
        assert_eq!(lints.len(), 2, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "The 'package:flutter/material.dart' library is deprecated."
        );
        assert_eq!(
            lints[0].correction.as_deref(),
            Some("Try using the Material or Cupertino package.")
        );
        let uri = tok(&f.source, &f.tokens, "'package:flutter/material.dart'", 0);
        assert_eq!(lints[0].range.start, utf16_at(&f, uri));
    }

    /// Any other import passes; an adjacent-string URI reads cooked.
    #[test]
    fn migrate_design_widgets_other_imports_pass() {
        let f = Fixture::parse(
            "import 'package:flutter/widgets.dart';\nimport 'package:meta/meta.dart';\nimport 'dart:async';\n",
        );
        assert!(f.run(&one_rule("migrate_design_widgets")).is_empty());

        let f = Fixture::parse("import 'package:flutter/' 'material.dart';\n");
        assert_eq!(
            f.run(&one_rule("migrate_design_widgets")).len(),
            1,
            "{:?}",
            f.run(&one_rule("migrate_design_widgets"))
        );
    }
}

/// The W5 (#1026) production-bridge pins: the directive-wave rules over real
/// parser trees, against the upstream corpora
/// (`pkg/linter/test/rules/<name>_test.dart`).
#[cfg(test)]
mod w5_pins {
    use crate::testsupport::{Fixture, one_rule};

    // —— library_names ———————————————————————————————————————————————————

    /// `test_titlecase` — a name off `lower_case_with_underscores` reports
    /// at the whole name with the cited text.
    #[test]
    fn library_names_reports_a_malformed_name() {
        let f = Fixture::parse("library Foo;\n");
        let lints = f.run(&one_rule("library_names"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "The library name 'Foo' isn't a lower_case_with_underscores identifier."
        );
        assert_eq!(lints[0].range, 8..11);
    }

    /// `test_lowercase` and `test_libraryWithoutName` — a compliant name
    /// and an unnamed `library;` pass.
    #[test]
    fn library_names_compliant_and_unnamed_pass() {
        let f = Fixture::parse("library foo;\n");
        assert!(f.run(&one_rule("library_names")).is_empty());

        let f = Fixture::parse("library;\n");
        assert!(f.run(&one_rule("library_names")).is_empty());
    }

    /// `test_uppercaseInDots` — a dotted name reports across every segment
    /// (the validator faces the whole dotted text).
    #[test]
    fn library_names_dotted_name_reports_once_over_all_segments() {
        let f = Fixture::parse("library one.Two.three;\n");
        let lints = f.run(&one_rule("library_names"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "The library name 'one.Two.three' isn't a lower_case_with_underscores identifier."
        );
        assert_eq!(lints[0].range, 8..21);
    }

    // —— package_prefixed_library_names ——————————————————————————————————

    /// Registered but permanently silent upstream (`_Visitor` body is
    /// `return;` — "Project info is not being set", dart-lang/linter#3395):
    /// parity is the no-op, named or not.
    #[test]
    fn package_prefixed_library_names_is_a_parity_noop() {
        let f = Fixture::parse("library foo.bar;\n");
        assert!(
            f.run(&one_rule("package_prefixed_library_names"))
                .is_empty()
        );

        let f = Fixture::parse("library Foo;\n");
        assert!(
            f.run(&one_rule("package_prefixed_library_names"))
                .is_empty()
        );
    }

    // —— directives_ordering —————————————————————————————————————————————

    /// `test_dartDirectivesGoFirst_imports` — every `dart:` directive after
    /// the leading dart: run reports; the alphabetical walk adds nothing
    /// (already-linted nodes are marked).
    #[test]
    fn directives_ordering_dart_uris_go_first() {
        let f = Fixture::parse(
            "import 'dart:math';\nimport 'a.dart';\nimport 'dart:html';\nimport 'dart:isolate';\n",
        );
        let lints = f.run(&one_rule("directives_ordering"));
        assert_eq!(lints.len(), 2, "{lints:?}");
        assert_eq!(lints[0].range.start, 37);
        assert_eq!(lints[1].range.start, 57);
        assert_eq!(
            lints[0].message,
            "Place 'dart:' imports before other imports."
        );
    }

    /// `test_importsGoBeforeExports` — exports before a later import
    /// report; the trailing export run is exempt.
    #[test]
    fn directives_ordering_exports_come_after_imports() {
        let f = Fixture::parse(
            "import 'a.dart';\n\nexport 'a.dart';\n\nimport 'b.dart';\n\nexport 'b.dart';\n\nimport 'c.dart';\n\nexport 'c.dart';\n",
        );
        let lints = f.run(&one_rule("directives_ordering"));
        assert_eq!(lints.len(), 2, "{lints:?}");
        assert_eq!(lints[0].range.start, 18);
        assert_eq!(lints[1].range.start, 54);
        assert_eq!(
            lints[0].message,
            "Specify exports in a separate section after all imports."
        );
    }

    /// `test_reportOneNodeOnlyOnce` — a directive violating two checks at
    /// once reports exactly once (the `lintedNodes` set).
    #[test]
    fn directives_ordering_reports_one_node_only_once() {
        let f = Fixture::parse(
            "import 'package:meta/meta.dart';\nimport 'a.dart';\nimport 'package:foo/foo.dart';\n",
        );
        let lints = f.run(&one_rule("directives_ordering"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(lints[0].range.start, 50);
        assert_eq!(
            lints[0].message,
            "Place 'package:' imports before relative imports."
        );
    }

    /// `test_multipleSchemaImportsSortedByPath` — a fully sorted
    /// multi-section file passes; `http:` URIs are absolute (the `:` face)
    /// and join no section.
    #[test]
    fn directives_ordering_sorted_sections_pass() {
        let f = Fixture::parse(
            "library lib;\n\nimport 'dart:aaa';\nimport 'dart:bbb';\n\nimport 'package:aaa/aaa.dart';\nimport 'package:bbb/bbb.dart';\n\nimport 'http://aaa.com';\nimport 'http://bbb.com';\n\nimport 'aaa/aaa.dart';\nimport 'bbb/bbb.dart';\n\nexport 'dart:aaa';\nexport 'dart:bbb';\n\nexport 'package:aaa/aaa.dart';\nexport 'package:bbb/bbb.dart';\n\nexport 'http://aaa.com';\nexport 'http://bbb.com';\n\nexport 'aaa/aaa.dart';\nexport 'bbb/bbb.dart';\n\npart 'aaa/aaa.dart';\npart 'bbb/bbb.dart';\n\nmain() {}\n",
        );
        assert!(f.run(&one_rule("directives_ordering")).is_empty());
    }
}

/// The #1097 W1 (#1102) production-bridge pins: the directive-wave rules
/// over real parser trees, against the upstream corpora
/// (`pkg/linter/test/rules/<name>_test.dart`).
#[cfg(test)]
mod wave1_pins {
    use std::path::{Path, PathBuf};

    use rustc_hash::FxHashSet;

    use crate::WorkspaceFacts;
    use crate::registry::rule_by_name;
    use crate::testsupport::{Fixture, one_rule, tok, utf16_at};

    /// The facts for a package at `root` named `own` with dependencies
    /// `deps` (no dev dependencies on this wave's faces).
    fn facts(own: &str, deps: &[&str], root: &Path) -> WorkspaceFacts {
        let mut dependencies: FxHashSet<String> =
            deps.iter().map(|name| (*name).to_owned()).collect();
        dependencies.insert(own.to_owned());
        WorkspaceFacts {
            own_package: own.to_owned(),
            dependencies,
            dev_dependencies: FxHashSet::default(),
            root: root.to_owned(),
            lib_dir: root.join("lib"),
        }
    }

    /// One reported hit as `(start, end, message)`.
    fn hits(f: &Fixture, rule: &str) -> Vec<(u32, u32, String)> {
        let id = rule_by_name(rule).expect("registered rule");
        f.run(&one_rule(rule))
            .into_iter()
            .map(|l| {
                assert_eq!(l.rule, id);
                (l.range.start, l.range.end, l.message)
            })
            .collect()
    }

    /// [`hits`] with `facts` mounted and the fixture's analyzed path set
    /// to `path` — the pub-aware rules' dispatch face.
    fn workspace_hits(
        f: &mut Fixture,
        path: &str,
        rule: &str,
        facts: &WorkspaceFacts,
    ) -> Vec<(u32, u32, String)> {
        f.path = PathBuf::from(path);
        let id = rule_by_name(rule).expect("registered rule");
        f.run_workspace(&one_rule(rule), facts)
            .into_iter()
            .map(|l| {
                assert_eq!(l.rule, id);
                (l.range.start, l.range.end, l.message)
            })
            .collect()
    }

    // —— simple_directive_paths ——————————————————————————————————————————

    /// `test_import_relative_nonMinimal` — the dot-relative URI reports at
    /// the literal with the official text.
    #[test]
    fn simple_directive_paths_dot_relative_reports() {
        let mut f = Fixture::parse("import './a.dart';\n");
        f.path = PathBuf::from("/pkg/lib/a.dart");
        let h = hits(&f, "simple_directive_paths");
        assert_eq!(h.len(), 1, "{h:?}");
        assert_eq!(h[0].2, "Use simple directive paths.");
        let uri = tok(&f.source, &f.tokens, "'./a.dart'", 0);
        assert_eq!(h[0].0, utf16_at(&f, uri));
    }

    /// `test_import_relative_minimal` and the package/dart faces —
    /// normalized URIs pass (the minimal-relative comparison is lexical:
    /// no file system is touched).
    #[test]
    fn simple_directive_paths_normalized_uris_pass() {
        let mut f = Fixture::parse(
            "import 'a.dart';\nimport '../shared/b.dart';\nimport 'package:x/y.dart';\nimport 'dart:math';\n",
        );
        f.path = PathBuf::from("/pkg/lib/src/a.dart");
        assert!(hits(&f, "simple_directive_paths").is_empty());
    }

    /// `test_partOf` — the URI form reports, the dotted-name form is
    /// silent (no string URI to read).
    #[test]
    fn simple_directive_paths_part_of_forms() {
        let mut f = Fixture::parse("part of './lib.dart';\n");
        f.path = PathBuf::from("/pkg/lib/b.dart");
        assert_eq!(hits(&f, "simple_directive_paths").len(), 1);

        let mut f = Fixture::parse("part of lib;\n");
        f.path = PathBuf::from("/pkg/lib/b.dart");
        assert!(hits(&f, "simple_directive_paths").is_empty());
    }

    /// `test_import_conditional` — the clause candidate is checked, the
    /// primary URI is not.
    #[test]
    fn simple_directive_paths_conditional_candidate_reports() {
        let mut f = Fixture::parse("export 'a.dart' if (dart.library.io) './b.dart';\n");
        f.path = PathBuf::from("/pkg/lib/a.dart");
        let h = hits(&f, "simple_directive_paths");
        assert_eq!(h.len(), 1, "{h:?}");
        let uri = tok(&f.source, &f.tokens, "'./b.dart'", 0);
        assert_eq!(h[0].0, utf16_at(&f, uri));
    }

    // —— always_use_package_imports ——————————————————————————————————————

    /// `test_samePackage_relativeUri` — relative URIs inside `lib/` report
    /// at the URI, the empty spelling included (its scheme is empty).
    #[test]
    fn always_use_package_imports_relative_uris_report_in_lib() {
        let own = facts("own", &[], Path::new("/pkg"));
        let mut f = Fixture::parse("import '../lib/b.dart';\nimport 'a.dart';\nimport '';\n");
        let h = workspace_hits(
            &mut f,
            "/pkg/lib/a.dart",
            "always_use_package_imports",
            &own,
        );
        assert_eq!(h.len(), 3, "{h:?}");
        assert_eq!(
            h[0].2,
            "Use 'package:' imports for files in the 'lib' directory."
        );
        let uri = tok(&f.source, &f.tokens, "'../lib/b.dart'", 0);
        assert_eq!(h[0].0, utf16_at(&f, uri));
    }

    /// `test_samePackage_packageSchema` and the gates: <package:/dart:/any>
    /// scheme passes, outside `lib/` the rule never registers
    /// (`test_samePackage_relativeUri_fromOutsideLib`), no workspace is
    /// the no-package posture, and an interpolated URI has no
    /// `stringValue`.
    #[test]
    fn always_use_package_imports_scheme_and_gate_faces() {
        let own = facts("own", &[], Path::new("/pkg"));
        let mut f = Fixture::parse(
            "import 'package:own/own.dart';\nimport 'dart:math';\nimport 'p:b.dart';\n",
        );
        assert!(
            workspace_hits(
                &mut f,
                "/pkg/lib/a.dart",
                "always_use_package_imports",
                &own
            )
            .is_empty()
        );
        let mut out = Fixture::parse("import '../lib/b.dart';\n");
        assert!(
            workspace_hits(
                &mut out,
                "/pkg/test/t.dart",
                "always_use_package_imports",
                &own
            )
            .is_empty()
        );
        let bare = Fixture::parse("import 'a.dart';\n");
        assert!(
            hits(&bare, "always_use_package_imports").is_empty(),
            "no workspace: the rule never registers"
        );
        let mut interpolated = Fixture::parse("import '$x.dart';\n");
        assert!(
            workspace_hits(
                &mut interpolated,
                "/pkg/lib/a.dart",
                "always_use_package_imports",
                &own
            )
            .is_empty()
        );
    }

    // —— prefer_relative_imports —————————————————————————————————————————

    /// `test_samePackage_packageSchema` — an own-package `package:` import
    /// from `lib/` reports; a dependency and a relative URI pass.
    #[test]
    fn prefer_relative_imports_own_package_reports() {
        let own = facts("own", &["meta"], Path::new("/pkg"));
        let mut f = Fixture::parse(
            "import 'package:own/own.dart';\nimport 'package:meta/meta.dart';\nimport '../lib/other.dart';\n",
        );
        let h = workspace_hits(&mut f, "/pkg/lib/a.dart", "prefer_relative_imports", &own);
        assert_eq!(h.len(), 1, "{h:?}");
        assert_eq!(
            h[0].2,
            "Use relative imports for files in the 'lib' directory."
        );
    }

    /// `test_samePackage_packageSchema_fromOutsideLib` — outside `lib/`
    /// the rule never registers; exports are not visited (the official
    /// registers the import directive only); no workspace stays silent.
    #[test]
    fn prefer_relative_imports_gates() {
        let own = facts("own", &[], Path::new("/pkg"));
        let mut out = Fixture::parse("import 'package:own/own.dart';\n");
        assert!(
            workspace_hits(
                &mut out,
                "/pkg/bin/main.dart",
                "prefer_relative_imports",
                &own
            )
            .is_empty()
        );
        let mut export = Fixture::parse("export 'package:own/own.dart';\n");
        assert!(
            workspace_hits(
                &mut export,
                "/pkg/lib/a.dart",
                "prefer_relative_imports",
                &own
            )
            .is_empty()
        );
        let bare = Fixture::parse("import 'package:own/own.dart';\n");
        assert!(
            hits(&bare, "prefer_relative_imports").is_empty(),
            "no workspace: the rule never registers"
        );
    }

    // —— avoid_web_libraries_in_flutter ———————————————————————————————————

    /// The web spellings report at the import directive (the conservative
    /// in-flutter default: any known package counts as a non-web Flutter
    /// package); near spellings and other libraries pass.
    #[test]
    fn avoid_web_libraries_reports_the_web_spellings() {
        let own = facts("own", &[], Path::new("/pkg"));
        let mut f = Fixture::parse(
            "import 'dart:html';\nimport 'dart:js';\nimport 'dart:js_util';\nimport 'dart:core';\nimport 'dart:html2';\n",
        );
        let h = workspace_hits(
            &mut f,
            "/pkg/lib/a.dart",
            "avoid_web_libraries_in_flutter",
            &own,
        );
        assert_eq!(h.len(), 3, "{h:?}");
        assert_eq!(
            h[0].2,
            "Don't use web-only libraries outside Flutter web plugins."
        );
        let import = tok(&f.source, &f.tokens, "import", 0);
        assert_eq!(h[0].0, utf16_at(&f, import), "the anchor is the directive");
    }

    /// The no-package gate stays silent (the official
    /// `package?.root == null` posture) and an interpolated spelling has
    /// no `stringValue`.
    #[test]
    fn avoid_web_libraries_gates() {
        let bare = Fixture::parse("import 'dart:html';\n");
        assert!(
            hits(&bare, "avoid_web_libraries_in_flutter").is_empty(),
            "no workspace: the rule never registers"
        );
        let own = facts("own", &[], Path::new("/pkg"));
        let mut interpolated = Fixture::parse("import 'dart:$x';\n");
        assert!(
            workspace_hits(
                &mut interpolated,
                "/pkg/lib/a.dart",
                "avoid_web_libraries_in_flutter",
                &own
            )
            .is_empty()
        );
    }

    // —— conditional_uri_does_not_exist ———————————————————————————————————

    /// `test_missingFiles` — a missing relative candidate reports at the
    /// clause literal with the candidate in the message; an existing one
    /// passes.
    #[test]
    fn conditional_uri_missing_relative_reports() {
        let dir = std::env::temp_dir().join("dartq_w1_cond_missing");
        std::fs::create_dir_all(&dir).expect("temp dir");
        std::fs::write(dir.join("exists.dart"), "library e;").expect("fixture file");
        let mut f = Fixture::parse(
            "import 'default.dart' if (dart.library.io) 'missing_1.dart' if (dart.library.html) 'exists.dart';\n",
        );
        f.path = dir.join("a.dart");
        let h = hits(&f, "conditional_uri_does_not_exist");
        assert_eq!(h.len(), 1, "{h:?}");
        assert_eq!(
            h[0].2,
            "The target of the conditional URI 'missing_1.dart' doesn't exist."
        );
        let uri = tok(&f.source, &f.tokens, "'missing_1.dart'", 0);
        assert_eq!(h[0].0, utf16_at(&f, uri));
        let _ = std::fs::remove_file(dir.join("exists.dart"));
    }

    /// The documented out-of-face forms stay silent: `dart:`/`package:`
    /// candidates (no SDK/package tables here), the empty recovery splice,
    /// an interpolated candidate.
    #[test]
    fn conditional_uri_skipped_forms_stay_silent() {
        let dir = std::env::temp_dir().join("dartq_w1_cond_skipped");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let mut f = Fixture::parse(
            "import 'd.dart' if (x) 'dart:missing' if (y) 'package:nope/nope.dart' if (z) '' if (w) '$v.dart';\n",
        );
        f.path = dir.join("a.dart");
        assert!(hits(&f, "conditional_uri_does_not_exist").is_empty());
    }

    /// An `==` comparison value is not the candidate — the report names
    /// the configured URI, not the compared constant.
    #[test]
    fn conditional_uri_eq_value_is_not_the_candidate() {
        let dir = std::env::temp_dir().join("dartq_w1_cond_eq");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let mut f = Fixture::parse("import 'd.dart' if (m == 'value') 'missing_2.dart';\n");
        f.path = dir.join("a.dart");
        let h = hits(&f, "conditional_uri_does_not_exist");
        assert_eq!(h.len(), 1, "{h:?}");
        assert_eq!(
            h[0].2,
            "The target of the conditional URI 'missing_2.dart' doesn't exist."
        );
    }
}
