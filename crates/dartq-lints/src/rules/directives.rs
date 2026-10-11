//! The directive family (the #300 parser milestone face):
//! `avoid_relative_lib_imports`, `library_annotations`, `library_prefixes`,
//! `no_leading_underscores_for_library_prefixes`, `unnecessary_library_name`,
//! `use_string_in_part_of_directives`.
//!
//! The payload contracts are collapse map §3.1: `Import.as/deferred`
//! (`ImportPrefixHandle` back-fill), `LibraryName.has_name`,
//! `PartOf.of/has_name`. Annotations attach as `Metadata` nodes under a
//! `MetadataStar` container that precedes the annotated node (the parser
//! opens declaration nodes after `parseMetadataStar`), so `library_annotations`
//! reads the annotated node as the star's next sibling.

use dartq_ast::NodeKind;

use crate::engine::Check;
use crate::registry::{Group, RuleSpec};

/// The node behind a directive prelude wrapper —
/// `begin_uncategorized_top_level_declaration` opens `TopLevelDeclaration`
/// around every directive and the five class-family preludes (collapse map
/// §2 #1-5), so a unit child that is a wrapper is read through its first
/// child.
fn behind_prelude(c: &Check<'_>, node: u32) -> u32 {
    if c.kind(node) != NodeKind::TopLevelDeclaration {
        return node;
    }
    c.collect_direct(node).first().copied().unwrap_or(node)
}

/// The payload bool word (`bool as u32`; a missing word reads false).
fn payload_flag(c: &Check<'_>, node: u32, word: usize) -> bool {
    c.payload(node).get(word).copied().unwrap_or(0) != 0
}

/// The import prefix identifier — the only direct [`NodeKind::Identifier`]
/// child of an [`NodeKind::Import`] (combinator names live inside Show/Hide
/// nodes, conditional-URI dotted names inside `ConditionalUri`).
fn import_prefix(c: &Check<'_>, node: u32) -> Option<u32> {
    c.collect_direct(node)
        .into_iter()
        .find(|&k| c.kind(k) == NodeKind::Identifier)
}

/// The URI of an import: the leading run of `StringLiteral` direct children
/// (the adjacent-string concatenation face — `AdjacentStringLiteralsHandle`
/// is D1, consecutive segments in the parent, collapse map §3.5).
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
    use super::{Check, Group, NodeKind, RuleSpec};

    /// Official: `pkg/linter/lib/src/rules/avoid_relative_lib_imports.dart:23-49`
    /// — a relative import URI whose path contains `/lib/` reports at the
    /// URI. `isRelativeLibImport` (:24-35): `Uri.tryParse(uri.stringValue)`,
    /// scheme empty, `uri.path.contains('/lib/')`. Message:
    /// `diagnostic.g.dart` `avoidRelativeLibImports`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "avoid_relative_lib_imports",
        group: Group::Core,
        problem: "Can't use a relative path to import a library in 'lib'.",
        correction: "Try fixing the relative path or changing the import to a 'package:' import.",
        kinds: &[NodeKind::Import],
        check,
    };

    /// The scheme prefix length (`letter (letter | digit | + - .)* ':'`, the
    /// `Uri` scheme grammar) — non-empty schemes disqualify (:28-32).
    fn scheme_prefix_len(value: &str) -> Option<usize> {
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
    use super::{Check, Group, NodeKind, RuleSpec};

    /// Official: `pkg/linter/lib/src/rules/library_annotations.dart:29-95` —
    /// a library-targeted annotation attached where it does not belong
    /// reports at the annotation. The unit walk (:48-58) stops at a
    /// `part of`, skips the `library` directive itself, and checks every
    /// other directive plus every declaration.
    ///
    /// The official face splits in two. The `isPragmaLateTrust` branch
    /// (:84-95) is syntax-visible and ported here. The `targetKinds` branch
    /// (:66-73) needs the element model (the annotation declaration's
    /// `@Target` metadata, resolved) and is not part of this slice — its
    /// blocked cases (e.g. `@TestOn('browser')` on the first import) need
    /// the resolution milestone.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "library_annotations",
        group: Group::Core,
        problem: "This annotation should be attached to a library directive.",
        correction: "Try attaching the annotation to a library directive.",
        kinds: &[NodeKind::Metadata],
        check,
    };

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
        // TopLevelDeclaration prelude (collapse map §2 #1-5), so read each
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
        let annotated = super::behind_prelude(c, annotated);
        // The `library` directive is never checked (:54-55).
        if c.kind(annotated) == NodeKind::LibraryName {
            return;
        }
        if !is_pragma_late_trust(c, c.node()) {
            return;
        }
        let range = c.utf16_span(c.node());
        c.report(range, &[]);
    }

    /// The syntax face of `isPragmaLateTrust` (:84-95): the official pins
    /// the dart:core `pragma` constructor through the element model. The
    /// written face: the annotation carries a single unprefixed `pragma`
    /// name — a prefixed `foo.pragma` carries two name identifiers and
    /// never matches (the official resolves only the unprefixed core
    /// constructor), and a same-file `pragma` declaration bails (the
    /// element identity, the conservative family). Exactly one positional
    /// string argument `'dart2js:late:trust'`; the named `name:` form is a
    /// compile error upstream (the factory takes one positional) and never
    /// fires.
    fn is_pragma_late_trust(c: &Check<'_>, meta: u32) -> bool {
        let mut name = None;
        let mut arguments = None;
        for kid in c.collect_direct(meta) {
            match c.kind(kid) {
                NodeKind::Identifier if name.is_none() => name = Some(kid),
                // A second name segment (`foo.pragma`) is a prefix chain.
                NodeKind::Identifier | NodeKind::Qualified => return false,
                NodeKind::Arguments => arguments = Some(kid),
                _ => {}
            }
        }
        let Some(name) = name else {
            return false;
        };
        if c.lexeme(c.head_token(name)) != "pragma" {
            return false;
        }
        if crate::rules::expressions::same_file::declares_name(c, "pragma") {
            return false;
        }
        let Some(arguments) = arguments else {
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
    use super::{Check, Group, NodeKind, RuleSpec};

    /// Official: `pkg/linter/lib/src/rules/library_prefixes.dart:31-56` — an
    /// import prefix must be `lowercase_with_underscores`:
    /// `isValidLibraryPrefix` (`pkg/linter/lib/src/utils.dart:58`,
    /// `^\$?_*[a-z][_a-z\d]*$`), the `_` wildcard exempt when
    /// `wildcard_variables` is on (:36-39). Message: `diagnostic.g.dart`
    /// `libraryPrefixes`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "library_prefixes",
        group: Group::Recommended,
        problem: "The prefix '{0}' isn't a lower_case_with_underscores identifier.",
        correction: "Try changing the prefix to follow the lower_case_with_underscores style.",
        kinds: &[NodeKind::Import],
        check,
    };

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
    use super::{Check, Group, NodeKind, RuleSpec};

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
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "no_leading_underscores_for_library_prefixes",
        group: Group::Recommended,
        problem: "The library prefix '{0}' starts with an underscore.",
        correction: "Try renaming the prefix to not start with an underscore.",
        kinds: &[NodeKind::Import],
        check,
    };

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

pub(crate) mod unnecessary_library_name {
    use super::{Check, Group, NodeKind, RuleSpec};

    /// Official: `pkg/linter/lib/src/rules/unnecessary_library_name.dart:28-44`
    /// — a named `library` directive reports at the name. The rule registers
    /// only with the `unnamed_libraries` feature (shipped in 2.19,
    /// `tools/experimental_features.yaml` :397-400; :32-34). The name's
    /// presence is the `LibraryName.has_name` payload (collapse map §3.1,
    /// `end_library_name`). Message: `diagnostic.g.dart`
    /// `unnecessaryLibraryName`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "unnecessary_library_name",
        group: Group::Recommended,
        problem: "Library names are not necessary.",
        correction: "Remove the library name.",
        kinds: &[NodeKind::LibraryName],
        check,
    };

    fn check(c: &mut Check<'_>) {
        if !c.unit.features.unnamed_libraries {
            return;
        }
        if !super::payload_flag(c, c.node(), 0) {
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
    use super::{Check, Group, NodeKind, RuleSpec};

    /// Official: `pkg/linter/lib/src/rules/use_string_in_part_of_directives.dart:29-47`
    /// — a `part of` in the dotted-name form reports at the directive. The
    /// rule registers only without the `enhanced_parts` feature (experimental,
    /// unshipped — `enhanced-parts` has no `enabledIn`,
    /// `tools/experimental_features.yaml` :150-152; :32-34). The form is the
    /// `PartOf.has_name` payload (collapse map §3.1, `end_part_of`).
    /// Message: `diagnostic.g.dart` `useStringInPartOfDirectives`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "use_string_in_part_of_directives",
        group: Group::Core,
        problem: "The part-of directive uses a library name.",
        correction: "Try converting the directive to use the URI of the library.",
        kinds: &[NodeKind::PartOf],
        check,
    };

    fn check(c: &mut Check<'_>) {
        if c.unit.features.enhanced_parts {
            return;
        }
        if !super::payload_flag(c, c.node(), 1) {
            return;
        }
        let range = c.utf16_span(c.node());
        c.report(range, &[]);
    }
}

pub(crate) mod provide_deprecation_message {
    use super::{Check, Group, NodeKind, RuleSpec};

    /// Official: `pkg/linter/lib/src/rules/provide_deprecation_message.dart:32-42`
    /// — an annotation whose element is the dart:core `deprecated` constant
    /// and which carries no argument list reports at the annotation.
    ///
    /// Approximation (documented): the element pin is taken from the name —
    /// a single unprefixed `deprecated` (a prefixed `foo.deprecated`
    /// resolves through an import prefix whose target is unknown here and
    /// never matches; the production slice carries the prefix chain as a
    /// second name identifier), bailing when a same-file declaration
    /// shadows the name. `@Deprecated` — the class, not the constant —
    /// stays silent per the element pin (`isDeprecated`, :37-42); the
    /// oracle reports it only under a compile error (a bare class
    /// annotation is always `no_annotation_constructor_arguments`), a
    /// face unreachable on valid code. Message:
    /// `diagnostic.g.dart` `provideDeprecationMessage`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "provide_deprecation_message",
        group: Group::Core,
        problem: "Missing a deprecation message.",
        correction: "Try using the constructor to provide a message ('@Deprecated(\"message\")').",
        kinds: &[NodeKind::Metadata],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        let kids = c.collect_direct(node);
        // `node.arguments == null` (:40). A bare `@deprecated` emits
        // `NoArguments`; `@deprecated()` keeps a real one and passes.
        if kids.iter().any(|&kid| c.kind(kid) == NodeKind::Arguments) {
            return;
        }
        // `elementAnnotation.isDeprecated` (:37-42): the unprefixed core
        // constant — a single name identifier; a second one is a prefix
        // chain (`foo.deprecated`) whose target is not the constant.
        let mut name = None;
        for kid in kids {
            match c.kind(kid) {
                NodeKind::Identifier if name.is_none() => name = Some(kid),
                NodeKind::Identifier | NodeKind::Qualified => return,
                _ => {}
            }
        }
        let Some(name) = name else {
            return;
        };
        if c.lexeme(c.head_token(name)) != "deprecated" {
            return;
        }
        if crate::rules::expressions::same_file::declares_name(c, "deprecated") {
            return;
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

    use super::{Check, Group, NodeKind, RuleSpec, written_uri};

    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "implementation_imports",
        group: Group::Recommended,
        problem: "Import of a library in the 'lib/src' directory of another package.",
        correction: "Try importing a public library that exports this library, or removing the import.",
        kinds: &[NodeKind::Import],
        check,
    };

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
    /// (collapse map §3.1 `ImportPrefixHandle`), the prefix is the
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
                b.patch(0, 0); // MemberKind (§3.3; the value is irrelevant here)
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
    /// `foo.pragma` never matches (the official pins the unprefixed
    /// dart:core `pragma` constructor, library_annotations.dart:84-95).
    #[test]
    fn library_annotations_prefixed_pragma_is_inert() {
        let f = Fixture::parse("@pragma('dart2js:late:trust') class C {}");
        assert_eq!(hits(&f, "library_annotations").len(), 1);

        let f = Fixture::parse("@foo.pragma('dart2js:late:trust') class C {}");
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
    /// reports; a prefixed `foo.deprecated` never matches; `@Deprecated` —
    /// the class, not the constant (:37-42) — and an argument-carrying form
    /// pass; a same-file `deprecated` declaration bails.
    #[test]
    fn provide_deprecation_message_constant_faces() {
        let f = Fixture::parse("@deprecated int top = 0;");
        assert_eq!(hits(&f, "provide_deprecation_message").len(), 1);

        let f = Fixture::parse("@foo.deprecated int top = 0;");
        assert!(hits(&f, "provide_deprecation_message").is_empty());

        let f = Fixture::parse("@Deprecated int top = 0;");
        assert!(hits(&f, "provide_deprecation_message").is_empty());

        let f = Fixture::parse("@deprecated() int top = 0;");
        assert!(hits(&f, "provide_deprecation_message").is_empty());

        let f = Fixture::parse("const String deprecated = 'x';\n@deprecated int top = 0;");
        assert!(hits(&f, "provide_deprecation_message").is_empty());
    }
}
