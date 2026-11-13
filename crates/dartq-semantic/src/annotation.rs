//! package:meta annotation recognition (#705, #880): whether a
//! declaration's annotation is one of package:meta's own — the top-level
//! getter face (`@sealed`, W6's `mixin_on_sealed_class` /
//! `subtype_of_sealed_class` producers), the class face (`@literal` on a
//! constructor, W7's `non_const_call_to_literal_constructor` producer),
//! and the named-constructor face (`UseResult.unless`, W7's
//! `undefined_referenced_parameter` producer) — so those lanes are pure
//! verdict logic over the captured [`crate::model::Container::annotations`].
//!
//! # The official decision, mirrored
//!
//! The oracle resolves the annotation through the ordinary scope chain and
//! tests the resolved element (`_isPackageMetaGetter` /
//! `_isTopGetter`, analyzer `element.dart:2100-2130`): a `GetterElement` with
//! the getter's name whose library is named `meta`. The resolution step
//! itself is the scope chain (bare identifier: the file's top-level
//! declarations first, then the unprefixed import namespace,
//! combinator-filtered; prefixed identifier: the prefix's namespace,
//! combinator-filtered — `element/scope.dart`).
//!
//! This layer's mirror answers the same question from the import faces
//! (name-level; the graph carries no element identity):
//!
//! 1. the spelled name denotes the getter — bare (`@sealed`) or the
//!    segment after a prefix (`@meta.sealed`);
//! 2. the resolution face exists — bare: `lib` has an unprefixed import of
//!    a meta library and declares no local under the name (a local shadows
//!    the import, so the annotation resolves to the local, never to meta);
//!    prefixed: an import whose prefix matches;
//! 3. the target library is meta — the resolved import target's declared
//!    `library <name>;` name equals `'meta'` ([`LibraryGraph::name`]), the
//!    exact key the oracle's `_isPackageMetaGetter` tests
//!    (`element.dart:2100-2102`: `element.library.name == _metaLibName`,
//!    `_metaLibName` at `element.dart:1725`). A directive-less library's
//!    name is `''` (`library_builder.dart:750-763`) — a vendored meta copy
//!    that drops `library meta;` fails the test exactly as the oracle's
//!    `hasLiteral` does, and the `package:meta/…` URI is never consulted:
//!    the URI distinguishes neither a renamed real package nor a vendored
//!    stand-in without the directive;
//! 4. the import's combinators allow the getter name (a `show`/`hide`
//!    filters the oracle's namespace build too).
//!
//! A class-based annotation is the same mirror with the class name in the
//! denoted slot (`@literal`); a named-constructor annotation adds one
//! selector segment — `@UseResult.unless`, or `@meta.UseResult.unless`
//! prefixed — where the head resolves through the import faces and the
//! selector names the constructor as written. The `unless` argument walk
//! mirrors `annotation_verifier.dart:706-710`: the string value is the
//! caller's const-eval capture, the report anchor is the written
//! expression under the `parameterDefined` label.
//!
//! Known limit (under-report direction, no fixture face): a getter
//! re-exported through a barrel library is not recognized — the import
//! target's uri must itself be `package:meta/…`, not merely transitively
//! export it.

use dartq_resolution::{LibraryGraph, LibraryId, LocalLookup, Lookup, NamespaceEntry, Namespaces};
use dartq_source::{FileId, SourceRange, StringId, StringTable};

use crate::model::Annotation;

/// The spelling-level form of [`is_meta_getter`]: whether a written
/// annotation spelling (as captured on a declaration site) names
/// package:meta's `getter` for `lib` — the decl-site walk's gate. The
/// site model keeps bare spellings, not resolved annotations, so the
/// shadow/import legs of [`meta_face`] run against the raw text.
pub fn is_meta_spelling(
    spelling: &str,
    getter: &str,
    strings: &StringTable,
    graph: &LibraryGraph,
    lib: LibraryId,
) -> bool {
    meta_face(spelling, getter, strings, graph, lib)
}

/// Whether `annotation` is the package:meta top-level getter named
/// `getter` for `lib`, per the import faces of `graph`.
pub fn is_meta_getter(
    annotation: &Annotation,
    getter: &str,
    strings: &StringTable,
    graph: &LibraryGraph,
    lib: LibraryId,
) -> bool {
    meta_face(
        strings.resolve(annotation.name),
        getter,
        strings,
        graph,
        lib,
    )
}

/// Whether `annotation` is package:meta's `literal` class for `lib` —
/// the `@literal` constructor annotation, bare or prefixed as written —
/// per the import faces of `graph`.
pub fn is_meta_literal(
    annotation: &Annotation,
    strings: &StringTable,
    graph: &LibraryGraph,
    lib: LibraryId,
) -> bool {
    meta_face(
        strings.resolve(annotation.name),
        "literal",
        strings,
        graph,
        lib,
    )
}

/// Whether `annotation` is the package:meta class named `class` for `lib`
/// — the unnamed- and named-constructor annotation face (`@UseResult`,
/// `@UseResult.unless`), bare or prefixed as written — per the import
/// faces of `graph`.
pub fn is_meta_class(
    annotation: &Annotation,
    class: &str,
    strings: &StringTable,
    graph: &LibraryGraph,
    lib: LibraryId,
) -> bool {
    meta_face(strings.resolve(annotation.name), class, strings, graph, lib)
}

/// One written argument of an annotation invocation, as captured by the
/// declaration pass.
pub struct AnnotationArg {
    /// The written label (`parameterDefined:`), `None` for a positional
    /// argument.
    pub label: Option<StringId>,
    /// The value face: the argument's cooked string value (a string
    /// literal's cooked text) plus the written expression's own span (a
    /// literal's span includes the quotes) — the official report anchor.
    /// `None` when the value is absent or not a string literal.
    pub string: Option<(StringId, SourceRange)>,
    /// The written face of a bare reference argument: the source text plus
    /// the written span. The official walk messages this text for a
    /// non-literal argument (the `toString` fallback,
    /// `annotation_verifier.dart:534-537`) while the gate value resolves
    /// from the caller's const environment.
    pub reference: Option<(StringId, SourceRange)>,
}

/// The `parameterDefined` face of a recognized `@UseResult.unless`.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum UnlessFace {
    /// A string-literal argument: the cooked value is both the official
    /// gate (`computeConstantValue`'s `toStringValue`) and the message
    /// face (the `SimpleStringLiteral` arm), the span the written literal.
    Literal { value: StringId, span: SourceRange },
    /// A reference argument: the written name is the message face (the
    /// `toString` fallback); the gate value resolves from the const
    /// environment at the check.
    Reference { name: StringId, span: SourceRange },
}

/// The `parameterDefined` face of `@UseResult.unless` from package:meta:
/// `Some(face)` when `annotation` resolves to the named constructor
/// `unless` of meta's `UseResult` class for `lib` and `args` carries a
/// `parameterDefined` argument. `None` for every other shape: an
/// unresolved import, a non-meta same-name resolution, the unnamed
/// `UseResult` face, another selector, or a missing `parameterDefined`.
pub fn use_result_unless(
    annotation: &Annotation,
    args: &[AnnotationArg],
    strings: &StringTable,
    graph: &LibraryGraph,
    lib: LibraryId,
) -> Option<UnlessFace> {
    // The written name is head + selector (`UseResult.unless`) behind an
    // optional prefix (`meta.UseResult.unless`); a selector-less spelling
    // is the unnamed face and never carries `unless`.
    let spelling = strings.resolve(annotation.name);
    let (head, selector, prefix) = match spelling.rsplit_once('.') {
        None => return None,
        Some((rest, selector)) => match rest.rsplit_once('.') {
            None => (rest, selector, None),
            Some((prefix, head)) => (head, selector, Some(prefix)),
        },
    };
    if selector != "unless" || head != "UseResult" {
        return None;
    }
    let head_id = strings.intern(head);
    let resolved = match prefix {
        None => {
            matches!(graph.local(lib, head_id), LocalLookup::Absent)
                && imports_meta(graph, lib, None, head_id)
        }
        Some(prefix) => imports_meta(graph, lib, Some(strings.intern(prefix)), head_id),
    };
    if !resolved {
        return None;
    }
    // The official walk takes the written expression under the
    // `parameterDefined` label (`annotation_verifier.dart:706-710`); the
    // value face is the caller's const-eval capture, the written face the
    // source text of that expression.
    let label = strings.intern("parameterDefined");
    args.iter()
        .find_map(|arg| match (arg.label, &arg.string, &arg.reference) {
            (Some(l), Some((value, span)), _) if l == label => Some(UnlessFace::Literal {
                value: *value,
                span: *span,
            }),
            (Some(l), None, Some((name, span))) if l == label => Some(UnlessFace::Reference {
                name: *name,
                span: *span,
            }),
            _ => None,
        })
}

/// [`use_result_unless`] without the face: the written expression's span
/// only.
pub fn use_result_unless_parameter_defined(
    annotation: &Annotation,
    args: &[AnnotationArg],
    strings: &StringTable,
    graph: &LibraryGraph,
    lib: LibraryId,
) -> Option<SourceRange> {
    use_result_unless(annotation, args, strings, graph, lib).map(|face| match face {
        UnlessFace::Literal { span, .. } | UnlessFace::Reference { span, .. } => span,
    })
}

/// The shared name-level resolution mirror: whether `spelling` denotes
/// the package:meta symbol `name` for `lib` — bare (`@sealed`): `lib`
/// declares no local under the name (a local shadows the import) and has
/// an unprefixed meta import; prefixed (`@meta.sealed`): an import whose
/// prefix matches; the combinators allow `name` on both paths.
fn meta_face(
    spelling: &str,
    name: &str,
    strings: &StringTable,
    graph: &LibraryGraph,
    lib: LibraryId,
) -> bool {
    match spelling.split_once('.') {
        None => {
            spelling == name
                && matches!(graph.local(lib, strings.intern(name)), LocalLookup::Absent)
                && imports_meta(graph, lib, None, strings.intern(name))
        }
        Some((prefix, rest)) => {
            rest == name
                && imports_meta(
                    graph,
                    lib,
                    Some(strings.intern(prefix)),
                    strings.intern(name),
                )
        }
    }
}

/// Whether `lib` has an import whose prefix face is `prefix` (`None` =
/// unprefixed) targeting a meta library, with `getter_id` allowed through
/// the combinators.
fn imports_meta(
    graph: &LibraryGraph,
    lib: LibraryId,
    prefix: Option<StringId>,
    getter_id: StringId,
) -> bool {
    graph.import_edges(lib).iter().any(|edge| {
        edge.prefix == prefix
            && edge
                .target
                .is_some_and(|target| is_meta_library(graph, target))
            && edge.combinators.allows(getter_id)
    })
}

/// The resolved-target form of the meta test: the official check reads the
/// element's `library.name == 'meta'` (`_metaLibName`, `element.dart:1725`;
/// `_isPackageMetaGetter`, `element.dart:2100-2102`) — the declaring
/// library's `library meta;` directive name, not its URI and not the
/// package it ships in. [`LibraryGraph::name`] carries exactly that
/// directive text, `""` for a directive-less library — so a vendored meta
/// copy that drops `library meta;` (the lints-new corpus stand-in) is
/// unrecognized exactly as the oracle's `hasLiteral` is false there
/// (`pkg/linter` `prefer_const_constructors.dart` gates the lint on it).
fn is_meta_library(graph: &LibraryGraph, target: LibraryId) -> bool {
    graph.name(target) == "meta"
}

// —— the dart:core deprecation tag (#2910) ————————————————————————————————
//
// The `deprecated_member_use` face's element test: upstream
// `DeprecatedElementUsageSet.getTagInfo` (deprecated_member_use_verifier.dart
// :73-95) scans the used element's metadata for an annotation the oracle
// resolved to dart:core's `deprecated` getter or `Deprecated` class, and
// reads the message argument. The name-level mirror answers the same
// question from the declaring library's scope faces: dart:core rides every
// library as the lowest-priority implicit import, so a BARE `deprecated` /
// `Deprecated` spelling whose scope-chain resolution lands on dart:core — the
// tag itself — or on nothing at all IS the tag; every other spelling stays
// untagged.
//
// The two resolution outcomes are both live shapes, not alternatives: the
// engine's graph registers every `dart:` library as a node (generated
// name-only face, or the loaded SDK units) and wires the implicit import
// edge into every live library, so a bare spelling resolves INTO dart:core;
// a rig with no `dart:core` node resolves it to nothing. Testing only the
// miss shape silently turns the whole face off on the engine graph.

/// Whether `annotation`'s spelling is a bare dart:core deprecation tag
/// (`deprecated` getter or `Deprecated` class) that resolves to dart:core
/// in `lib`'s scope — the declaring library the annotation is written in.
pub fn is_core_deprecated(
    annotation: &Annotation,
    strings: &StringTable,
    graph: &LibraryGraph,
    namespaces: &Namespaces,
    lib: LibraryId,
) -> bool {
    is_core_deprecated_spelling(
        strings.resolve(annotation.name),
        strings,
        graph,
        namespaces,
        lib,
    )
}

/// [`is_core_deprecated`] over an already-resolved spelling — the
/// declaration-site frontier's form (the unit's [`crate::model::DeclSite`]
/// annotation names carry no [`Annotation`] face).
pub fn is_core_deprecated_spelling(
    spelling: &str,
    strings: &StringTable,
    graph: &LibraryGraph,
    namespaces: &Namespaces,
    lib: LibraryId,
) -> bool {
    // The prefixed face (`@core.deprecated` under `import 'dart:core' as
    // core;`): the segment after the dot names the tag and the written
    // prefix rides an import edge whose target is dart:core — the same
    // resolution the oracle's element walk answers (a `p.deprecated`
    // under any other prefix never resolves to the core tag).
    if let Some((prefix, name)) = spelling.split_once('.') {
        if name != "deprecated" && name != "Deprecated" {
            return false;
        }
        let prefix_id = strings.intern(prefix);
        let name_id = strings.intern(name);
        return graph.import_edges(lib).iter().any(|edge| {
            edge.prefix == Some(prefix_id)
                && edge
                    .target
                    .is_some_and(|target| graph.uri(target) == "dart:core")
                && edge.combinators.allows(name_id)
        });
    }
    if spelling != "deprecated" && spelling != "Deprecated" {
        return false;
    }
    let id = strings.intern(spelling);
    // A local declaration shadows the implicit dart:core import (the
    // annotation would resolve to it, never to the tag). The setter-only
    // face shadows too, and the namespace walk cannot tell that apart from
    // a miss (`LocalLookup::Shadowed` folds to `Lookup::NotFound` there).
    if matches!(graph.local(lib, id), LocalLookup::Shadowed) {
        return false;
    }
    match namespaces.lookup(graph, lib, id) {
        // The engine posture: the SDK's `dart:core` registers as a node
        // (generated face or loaded units) and every live library carries
        // the implicit import edge to it, so the tag resolves — through
        // that edge or an explicit `import 'dart:core'` — to dart:core's
        // own `deprecated` getter / `Deprecated` class. A hit in any other
        // library is a same-spelled declaration, not the tag.
        Lookup::Found(NamespaceEntry::Decl(file)) => is_dart_core_file(graph, file),
        Lookup::Found(NamespaceEntry::Prefix) | Lookup::Ambiguous(_) => false,
        // Nothing in scope resolves the name. On a graph that carries
        // `dart:core` that is a real miss — a `show`/`hide` (or an explicit
        // `dart:core` import) filtered the tag out of the implicit edge, and
        // an annotation that resolves to no element is no tag. On a graph
        // with no `dart:core` node at all the implicit import is simply not
        // modelled (the rig posture), so the spelling IS the tag.
        Lookup::NotFound => !has_dart_core(graph),
    }
}

/// Whether `file` belongs to the SDK's `dart:core` library. A file with no
/// library row is not `dart:core`: the tag arm needs the declaring library's
/// identity.
fn is_dart_core_file(graph: &LibraryGraph, file: FileId) -> bool {
    graph
        .library_of_file(file)
        .is_some_and(|target| graph.uri(target) == "dart:core")
}

/// Whether the graph carries a `dart:core` node — the engine registers one
/// per `dart:` library, a hand-built rig usually registers none. A rig's
/// name-only stub (interned for the prefixed-edge face, carrying no unit
/// files) does not count: without mounted units the implicit import is not
/// modelled either, so the graph is in the rig posture however the node is
/// spelled. Cold path: only a name that resolves nowhere asks.
fn has_dart_core(graph: &LibraryGraph) -> bool {
    graph
        .libraries()
        .any(|lib| graph.uri(lib) == "dart:core" && !graph.unit_files(lib).is_empty())
}

// —— the deprecation KIND face (#3353) ————————————————————————————————————
//
// The `@Deprecated.<kind>` constructor family (upstream Dart SDK source,
// sdk/lib/core/annotations.dart: `Deprecated.extend`/`.implement`/
// `.instantiate`/`.mixin`/`.optional`/`.subclass`, each fixing
// `_kind = _DeprecationKind.<name>`, plus the unnamed constructor and the
// `deprecated` constant, which fix `use`). The oracle reads the kind off the
// annotation's computed constant — `ElementAnnotationImpl.deprecationKind`
// (element.dart:1785-1793: `computeConstantValue()?.getField('_kind')
// ?.getField('_name')?.toStringValue() ?? 'use'`) — and
// `isDeprecatedWithKind` (:2300-2302) intersects it with the element's
// deprecation tag (`isDeprecated`, :1818-1828: a `ConstructorElement` of
// dart:core's `Deprecated` class, or the `deprecated` top-level getter).
//
// At name level the SELECTOR the annotation spells IS that constant: the
// head must pass the tag gate above, and the selector names the constructor.
// No other spelling reaches a kind.

/// One [`_DeprecationKind`] of dart:core's `Deprecated` annotation.
///
/// [`_DeprecationKind`]: https://api.dart.dev/stable/dart-core/Deprecated-class.html
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeprecationKind {
    /// The unnamed `Deprecated(…)` constructor and the `deprecated`
    /// constant — the kind of every plain tag.
    Use,
    /// `@Deprecated.extend(…)` — extending the annotated class.
    Extend,
    /// `@Deprecated.implement(…)` — implementing it.
    Implement,
    /// `@Deprecated.instantiate(…)` — instantiating it.
    Instantiate,
    /// `@Deprecated.mixin(…)` — mixing it in.
    Mixin,
    /// `@Deprecated.optional(…)` — omitting an optional parameter.
    Optional,
    /// `@Deprecated.subclass(…)` — extending or implementing it.
    Subclass,
}

impl DeprecationKind {
    /// The kind a named constructor spells; `None` for every other selector
    /// (`use` is the unnamed face and has no named constructor).
    fn from_selector(selector: &str) -> Option<Self> {
        Some(match selector {
            "extend" => Self::Extend,
            "implement" => Self::Implement,
            "instantiate" => Self::Instantiate,
            "mixin" => Self::Mixin,
            "optional" => Self::Optional,
            "subclass" => Self::Subclass,
            _ => return None,
        })
    }
}

/// The deprecation kind `annotation` names for `lib` — the name-level
/// `ElementAnnotationImpl.deprecationKind`: the tag gate plus the written
/// constructor selector. `None` when the annotation is no dart:core
/// deprecation tag at all.
pub fn deprecation_kind(
    annotation: &Annotation,
    strings: &StringTable,
    graph: &LibraryGraph,
    namespaces: &Namespaces,
    lib: LibraryId,
) -> Option<DeprecationKind> {
    deprecation_kind_spelling(
        strings.resolve(annotation.name),
        strings,
        graph,
        namespaces,
        lib,
    )
}

/// [`deprecation_kind`] over an already-resolved spelling — the form the
/// declaration-site stars need (their names carry no [`Annotation`] face).
pub fn deprecation_kind_spelling(
    spelling: &str,
    strings: &StringTable,
    graph: &LibraryGraph,
    namespaces: &Namespaces,
    lib: LibraryId,
) -> Option<DeprecationKind> {
    match spelling.rsplit_once('.') {
        // Bare (`@deprecated`, `@Deprecated`): the unnamed face.
        None => is_core_deprecated_spelling(spelling, strings, graph, namespaces, lib)
            .then_some(DeprecationKind::Use),
        Some((head, selector)) => match DeprecationKind::from_selector(selector) {
            // `@Deprecated.extend` / `@core.Deprecated.extend`: the head
            // (with its written prefix) must be the dart:core tag.
            Some(kind) => {
                is_core_deprecated_spelling(head, strings, graph, namespaces, lib).then_some(kind)
            }
            // The dot is a PREFIX (`@core.deprecated`): the tag gate reads
            // the whole spelling and answers the unnamed face.
            None => is_core_deprecated_spelling(spelling, strings, graph, namespaces, lib)
                .then_some(DeprecationKind::Use),
        },
    }
}

/// The message face of a `@Deprecated` annotation: the cooked string of the
/// first positional / `message:`-labeled (then `expires:`-labeled) string
/// argument, normalized the way the oracle presents it
/// (`normalizeDeprecationMessage`, deprecated_member_use_verifier.dart:18-29
/// — trimmed; empty or a lone `.` means no message; a missing terminal
/// `.`/`?`/`!` gains one). `None` when the annotation carries no readable
/// message — the plain `deprecated_member_use` row.
#[must_use]
pub fn deprecation_message(args_text: Option<StringId>, strings: &StringTable) -> Option<String> {
    let args = strings.resolve(args_text?);
    let raw = deprecation_arg_string(args)?;
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed == "." {
        return None;
    }
    Some(match trimmed.as_bytes()[trimmed.len() - 1] {
        b'.' | b'?' | b'!' => trimmed.to_owned(),
        _ => format!("{trimmed}."),
    })
}

/// The first readable string-literal argument of a `@Deprecated` argument
/// group, cooked (`('Use x')` / `(message: 'Use x')` / `(expires: '2024')`):
/// the official read is `getField('message') ?? getField('expires')` over
/// the computed constant, so a labeled pair prefers `message`. `None` when
/// the group carries no string literal (a `@Deprecated()` or a
/// non-literal argument stays plain).
fn deprecation_arg_string(args: &str) -> Option<String> {
    let body = args.strip_prefix('(')?.strip_suffix(')')?;
    let mut positional = None;
    let mut expires = None;
    let mut rest = body;
    while let Some(start) = rest.find('\'').or_else(|| rest.find('"')) {
        let quote = rest.as_bytes()[start];
        // The label before the literal, when the literal sits under one:
        // the last identifier run between the previous delimiter and the
        // literal (a `message:`/`expires:` label reads back one token).
        let head = rest[..start].trim_end();
        let label = head
            .strip_suffix(':')
            .map_or("", |head| head.trim_start())
            .rsplit(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '$'))
            .next()
            .unwrap_or("");
        let cooked = cook_string_literal(&rest[start..], quote)?;
        let advance = start + cooked.len() + 2;
        match label {
            "message" => return Some(cooked),
            "expires" => expires = expires.or(Some(cooked)),
            _ => {
                if positional.is_none() && (head.is_empty() || head.ends_with(',')) {
                    positional = Some(cooked);
                }
            }
        }
        rest = &rest[advance..];
    }
    positional.or(expires)
}

/// Cooks the string literal at the head of `text` (whose first byte is the
/// opening `quote`): the raw content between the quotes with the common
/// escapes processed. `None` when the literal never closes.
fn cook_string_literal(text: &str, quote: u8) -> Option<String> {
    let mut out = String::new();
    let mut chars = text[1..].chars();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' => {
                let esc = chars.next()?;
                out.push(match esc {
                    'n' => '\n',
                    'r' => '\r',
                    't' => '\t',
                    other => other,
                });
            }
            c if c as u32 == u32::from(quote) => return Some(out),
            c => out.push(c),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use dartq_resolution::{CombinatorClause, Combinators, ImportEdge, LibraryGraphBuilder};
    use dartq_source::{FileTable, SourceRange};
    use std::path::Path;

    /// The shared rig: an analyzed library plus the imports and local
    /// declarations a pin needs, mirroring the engine's graph build
    /// (`package:` canonical uris, one interned node per library, the
    /// declared `library <name>;` name registered per target).
    struct Rig {
        strings: StringTable,
        graph: LibraryGraph,
        lib: LibraryId,
    }

    impl Rig {
        fn new(imports: &[Import], locals: &[&str]) -> Self {
            let mut strings = StringTable::new();
            let mut files = FileTable::new();
            // Pre-intern spellings and build the combinator lists while
            // `strings` is still exclusively ours (the builder holds a
            // shared borrow through `finish`).
            let edges: Vec<(
                &'static str,
                Option<&'static str>,
                Option<StringId>,
                Combinators,
            )> = imports
                .iter()
                .map(|import| {
                    (
                        import.uri,
                        import.name,
                        import.prefix.map(|p| strings.intern(p)),
                        import.combinators(&mut strings),
                    )
                })
                .collect();
            let locals: Vec<StringId> = locals.iter().map(|n| strings.intern(n)).collect();
            let mut builder = LibraryGraphBuilder::new(&strings);
            let lib = builder.intern("package:app/app.dart");
            for local in locals {
                let file = files.intern(Path::new("lib/app.dart"));
                builder.declare(lib, local, file);
            }
            for (uri, name, prefix, combinators) in edges {
                let target = builder.intern(uri);
                if let Some(name) = name {
                    builder.library_name(target, name);
                }
                builder.import(
                    lib,
                    ImportEdge {
                        target: Some(target),
                        prefix,
                        deferred: false,
                        non_library: false,
                        combinators,
                    },
                );
            }
            let graph = builder.finish();
            Self {
                strings,
                graph,
                lib,
            }
        }

        fn recognizes(&self, spelling: &str, getter: &str) -> bool {
            is_meta_getter(
                &Annotation {
                    name: self.strings.intern(spelling),
                    range: SourceRange {
                        offset: 0,
                        length: 8,
                    },
                    args_text: None,
                },
                getter,
                &self.strings,
                &self.graph,
                self.lib,
            )
        }

        fn meta_literal(&self, spelling: &str) -> bool {
            is_meta_literal(&self.ann(spelling), &self.strings, &self.graph, self.lib)
        }

        fn unless_span(&self, spelling: &str, args: &[AnnotationArg]) -> Option<SourceRange> {
            use_result_unless_parameter_defined(
                &self.ann(spelling),
                args,
                &self.strings,
                &self.graph,
                self.lib,
            )
        }

        fn unless_face(&self, spelling: &str, args: &[AnnotationArg]) -> Option<UnlessFace> {
            use_result_unless(
                &self.ann(spelling),
                args,
                &self.strings,
                &self.graph,
                self.lib,
            )
        }

        fn ann(&self, spelling: &str) -> Annotation {
            Annotation {
                name: self.strings.intern(spelling),
                range: SourceRange {
                    offset: 0,
                    length: 8,
                },
                args_text: None,
            }
        }

        /// A `parameterDefined:` argument with (or without) its string face.
        fn param_defined(&self, value: Option<(&'static str, SourceRange)>) -> AnnotationArg {
            self.labeled("parameterDefined", value)
        }

        fn labeled(
            &self,
            label: &'static str,
            value: Option<(&'static str, SourceRange)>,
        ) -> AnnotationArg {
            AnnotationArg {
                label: Some(self.strings.intern(label)),
                string: value.map(|(v, range)| (self.strings.intern(v), range)),
                reference: None,
            }
        }

        /// A bare reference argument (`parameterDefined: s`) with its
        /// written name and span.
        fn referenced(&self, name: &'static str, range: SourceRange) -> AnnotationArg {
            AnnotationArg {
                label: Some(self.strings.intern("parameterDefined")),
                string: None,
                reference: Some((self.strings.intern(name), range)),
            }
        }

        fn positional(&self, value: Option<(&'static str, SourceRange)>) -> AnnotationArg {
            AnnotationArg {
                label: None,
                string: value.map(|(v, range)| (self.strings.intern(v), range)),
                reference: None,
            }
        }
    }

    struct Import {
        uri: &'static str,
        /// The target's declared `library <name>;` name — `Some("meta")`
        /// mirrors the real package:meta's directive; `None` is the
        /// directive-less shape (the oracle's `''` library name).
        name: Option<&'static str>,
        prefix: Option<&'static str>,
        show: Option<&'static [&'static str]>,
        hide: Option<&'static [&'static str]>,
    }

    impl Import {
        fn unprefixed(uri: &'static str) -> Self {
            Self {
                uri,
                name: None,
                prefix: None,
                show: None,
                hide: None,
            }
        }

        /// An unprefixed import of a library named `name` — the
        /// `library <name>;` directive carries the oracle's identity.
        fn named(uri: &'static str, name: &'static str) -> Self {
            Self {
                name: Some(name),
                ..Self::unprefixed(uri)
            }
        }

        fn prefixed(uri: &'static str, prefix: &'static str) -> Self {
            Self {
                uri,
                name: None,
                prefix: Some(prefix),
                show: None,
                hide: None,
            }
        }

        /// A prefixed import of a library named `name`.
        fn prefixed_named(uri: &'static str, name: &'static str, prefix: &'static str) -> Self {
            Self {
                name: Some(name),
                ..Self::prefixed(uri, prefix)
            }
        }

        fn combinators(&self, strings: &mut StringTable) -> Combinators {
            let mut clauses = Vec::new();
            if let Some(names) = self.show {
                clauses.push(CombinatorClause::Show(
                    names.iter().map(|n| strings.intern(n)).collect(),
                ));
            }
            if let Some(names) = self.hide {
                clauses.push(CombinatorClause::Hide(
                    names.iter().map(|n| strings.intern(n)).collect(),
                ));
            }
            if clauses.is_empty() {
                Combinators::unfiltered()
            } else {
                Combinators::new(clauses, strings)
            }
        }
    }

    /// A bare `@sealed` under an unprefixed `package:meta/meta.dart`
    /// import recognizes — the corpus stand-in's exact face (the vendored
    /// pkg/meta resolves the same canonical uri). A different getter name
    /// on the same spelling does not.
    #[test]
    fn bare_sealed_under_meta_import_recognizes() {
        let rig = Rig::new(&[Import::named("package:meta/meta.dart", "meta")], &[]);
        assert!(rig.recognizes("sealed", "sealed"));
        assert!(!rig.recognizes("sealed", "literal"));
    }

    /// No import: the annotation resolves to nothing meta.
    #[test]
    fn no_import_does_not_recognize() {
        let rig = Rig::new(&[], &[]);
        assert!(!rig.recognizes("sealed", "sealed"));
    }

    /// An import of a non-meta package declaring its own top-level
    /// `sealed`: the library-name test rejects it — the getter identity is
    /// the resolved element's `library.name`, not the spelled name.
    #[test]
    fn same_named_getter_outside_meta_does_not_recognize() {
        let rig = Rig::new(&[Import::unprefixed("package:other/other.dart")], &[]);
        assert!(!rig.recognizes("sealed", "sealed"));
    }

    /// The oracle's key is `library.name == 'meta'`, not the URI
    /// (`element.dart:2100-2102`, `_metaLibName` at `element.dart:1725`):
    /// the lints-new corpus's vendored `vendor/meta/lib/meta.dart` drops
    /// `library meta;`, so a `package:meta/…` import of it must NOT
    /// recognize — the oracle's `hasLiteral` is false there and the
    /// `prefer_const_constructors` lint fires where a URI test would
    /// suppress it. Conversely a `library meta;` outside the meta package
    /// (a vendored or renamed copy keeping the directive) IS meta to the
    /// oracle.
    #[test]
    fn library_name_is_the_meta_key_not_the_uri() {
        // The vendored stand-in: package:meta URI, no `library meta;`
        // directive — unrecognized exactly as the oracle sees it.
        let vendored = Rig::new(&[Import::unprefixed("package:meta/meta.dart")], &[]);
        assert!(!vendored.recognizes("sealed", "sealed"));
        assert!(!vendored.meta_literal("literal"));

        // The name travels with the directive, not the package: a
        // `library meta;` copy under another URI still recognizes.
        let copied = Rig::new(&[Import::named("package:vendor/m.dart", "meta")], &[]);
        assert!(copied.recognizes("sealed", "sealed"));
        assert!(copied.meta_literal("literal"));

        // A different declared name on the same meta URI is not meta.
        let renamed = Rig::new(&[Import::named("package:meta/meta.dart", "not_meta")], &[]);
        assert!(!renamed.recognizes("sealed", "sealed"));
    }

    /// A local top-level declaration shadows the unprefixed import: the
    /// annotation resolves locally, never to meta.
    #[test]
    fn local_shadow_suppresses_bare_recognition() {
        let rig = Rig::new(
            &[Import::named("package:meta/meta.dart", "meta")],
            &["sealed"],
        );
        assert!(!rig.recognizes("sealed", "sealed"));
    }

    /// A prefixed import recognizes through its prefix (`@meta.sealed`
    /// under `import ... as meta;`); a bare spelling never crosses a
    /// prefixed import, and the wrong prefix leaves the getter
    /// unresolved.
    #[test]
    fn prefixed_import_recognizes_by_prefix() {
        let rig = Rig::new(
            &[Import::prefixed_named(
                "package:meta/meta.dart",
                "meta",
                "meta",
            )],
            &[],
        );
        assert!(rig.recognizes("meta.sealed", "sealed"));
        assert!(!rig.recognizes("sealed", "sealed"));
        assert!(!rig.recognizes("p.sealed", "sealed"));
    }

    /// A `show` filter hides the getter: the namespace the oracle builds
    /// never carries it, so the annotation cannot resolve to meta — while
    /// the shown getter keeps recognizing.
    #[test]
    fn hidden_getter_does_not_recognize() {
        let rig = Rig::new(
            &[Import {
                uri: "package:meta/meta.dart",
                name: Some("meta"),
                prefix: None,
                show: Some(&["literal"]),
                hide: None,
            }],
            &[],
        );
        assert!(!rig.recognizes("sealed", "sealed"));
        assert!(rig.recognizes("literal", "literal"));
    }

    // ---- is_meta_literal (the `@literal` class face) ----

    /// A bare `@literal` under an unprefixed meta import recognizes;
    /// another meta name on the same spelling does not.
    #[test]
    fn bare_literal_recognizes() {
        let rig = Rig::new(&[Import::named("package:meta/meta.dart", "meta")], &[]);
        assert!(rig.meta_literal("literal"));
        assert!(!rig.meta_literal("sealed"));
    }

    /// A prefixed spelling recognizes through its prefix (`@meta.literal`
    /// under `import ... as meta;`); the bare spelling never crosses a
    /// prefixed import, and a wrong prefix stays unresolved.
    #[test]
    fn prefixed_literal_recognizes_by_prefix() {
        let rig = Rig::new(
            &[Import::prefixed_named(
                "package:meta/meta.dart",
                "meta",
                "meta",
            )],
            &[],
        );
        assert!(rig.meta_literal("meta.literal"));
        assert!(!rig.meta_literal("literal"));
        assert!(!rig.meta_literal("p.literal"));
    }

    /// `show literal` keeps `@literal` recognizing while filtering
    /// `@sealed`; `hide literal` drops the face.
    #[test]
    fn literal_respects_combinators() {
        let shown = Rig::new(
            &[Import {
                uri: "package:meta/meta.dart",
                name: Some("meta"),
                prefix: None,
                show: Some(&["literal"]),
                hide: None,
            }],
            &[],
        );
        assert!(shown.meta_literal("literal"));
        assert!(!shown.meta_literal("sealed"));

        let hid = Rig::new(
            &[Import {
                uri: "package:meta/meta.dart",
                name: Some("meta"),
                prefix: None,
                show: None,
                hide: Some(&["literal"]),
            }],
            &[],
        );
        assert!(!hid.meta_literal("literal"));
    }

    /// No import: unresolved, no meta face, no panic.
    #[test]
    fn literal_without_import_does_not_recognize() {
        let rig = Rig::new(&[], &[]);
        assert!(!rig.meta_literal("literal"));
    }

    /// A same-name declaration outside a `meta`-named library is rejected
    /// by the library-name test.
    #[test]
    fn same_named_literal_outside_meta_does_not_recognize() {
        let rig = Rig::new(&[Import::unprefixed("package:other/other.dart")], &[]);
        assert!(!rig.meta_literal("literal"));
    }

    /// A local top-level declaration shadows the import.
    #[test]
    fn local_literal_shadow_suppresses_recognition() {
        let rig = Rig::new(
            &[Import::named("package:meta/meta.dart", "meta")],
            &["literal"],
        );
        assert!(!rig.meta_literal("literal"));
    }

    // ---- use_result_unless_parameter_defined (the `UseResult.unless` face) ----

    /// The anchor span asserted throughout: any literal span the caller
    /// captured comes back untouched.
    const SPAN: SourceRange = SourceRange {
        offset: 40,
        length: 3,
    };

    /// The canonical face: bare `@UseResult.unless(parameterDefined: 'p')`
    /// under an unprefixed meta import returns the written literal's span.
    #[test]
    fn bare_unless_resolves_parameter_defined_span() {
        let rig = Rig::new(&[Import::named("package:meta/meta.dart", "meta")], &[]);
        assert_eq!(
            rig.unless_span("UseResult.unless", &[rig.param_defined(Some(("p", SPAN)))]),
            Some(SPAN)
        );
    }

    /// Prefixed: `@meta.UseResult.unless` through `import ... as meta;`.
    #[test]
    fn prefixed_unless_resolves_parameter_defined_span() {
        let rig = Rig::new(
            &[Import::prefixed_named(
                "package:meta/meta.dart",
                "meta",
                "meta",
            )],
            &[],
        );
        assert_eq!(
            rig.unless_span(
                "meta.UseResult.unless",
                &[rig.param_defined(Some(("p", SPAN)))]
            ),
            Some(SPAN)
        );
    }

    /// `show UseResult` allows the face; `show literal` (`UseResult`
    /// filtered out) and `hide UseResult` both drop it.
    #[test]
    fn unless_respects_combinators() {
        let shown = Rig::new(
            &[Import {
                uri: "package:meta/meta.dart",
                name: Some("meta"),
                prefix: None,
                show: Some(&["UseResult"]),
                hide: None,
            }],
            &[],
        );
        assert_eq!(
            shown.unless_span(
                "UseResult.unless",
                &[shown.param_defined(Some(("p", SPAN)))]
            ),
            Some(SPAN)
        );

        let filtered = Rig::new(
            &[Import {
                uri: "package:meta/meta.dart",
                name: Some("meta"),
                prefix: None,
                show: Some(&["literal"]),
                hide: None,
            }],
            &[],
        );
        assert_eq!(
            filtered.unless_span(
                "UseResult.unless",
                &[filtered.param_defined(Some(("p", SPAN)))]
            ),
            None
        );

        let hid = Rig::new(
            &[Import {
                uri: "package:meta/meta.dart",
                name: Some("meta"),
                prefix: None,
                show: None,
                hide: Some(&["UseResult"]),
            }],
            &[],
        );
        assert_eq!(
            hid.unless_span("UseResult.unless", &[hid.param_defined(Some(("p", SPAN)))]),
            None
        );
    }

    /// Every unresolved face is `None`, never a panic: no import at all,
    /// a same-name `UseResult` outside meta, a local `UseResult` shadow.
    #[test]
    fn unless_unresolved_faces_are_none() {
        let absent = Rig::new(&[], &[]);
        assert_eq!(
            absent.unless_span(
                "UseResult.unless",
                &[absent.param_defined(Some(("p", SPAN)))]
            ),
            None
        );

        let other = Rig::new(&[Import::unprefixed("package:other/other.dart")], &[]);
        assert_eq!(
            other.unless_span(
                "UseResult.unless",
                &[other.param_defined(Some(("p", SPAN)))]
            ),
            None
        );

        let shadowed = Rig::new(
            &[Import::named("package:meta/meta.dart", "meta")],
            &["UseResult"],
        );
        assert_eq!(
            shadowed.unless_span(
                "UseResult.unless",
                &[shadowed.param_defined(Some(("p", SPAN)))]
            ),
            None
        );
    }

    /// The unnamed face (`@UseResult('reason')`), another selector, and a
    /// foreign head never fire.
    #[test]
    fn unless_requires_selector_and_head() {
        let rig = Rig::new(&[Import::named("package:meta/meta.dart", "meta")], &[]);
        assert_eq!(
            rig.unless_span("UseResult", &[rig.param_defined(Some(("p", SPAN)))]),
            None
        );
        assert_eq!(
            rig.unless_span("UseResult.foo", &[rig.param_defined(Some(("p", SPAN)))]),
            None
        );
        assert_eq!(
            rig.unless_span("Literal.unless", &[rig.param_defined(Some(("p", SPAN)))]),
            None
        );
    }

    /// A recognized annotation still needs its `parameterDefined` string:
    /// no argument, a positional argument, a non-matching label, and a
    /// non-string value all return `None`.
    #[test]
    fn unless_requires_string_parameter_defined() {
        let rig = Rig::new(&[Import::named("package:meta/meta.dart", "meta")], &[]);
        assert_eq!(rig.unless_span("UseResult.unless", &[]), None);
        assert_eq!(
            rig.unless_span("UseResult.unless", &[rig.positional(Some(("p", SPAN)))]),
            None
        );
        assert_eq!(
            rig.unless_span(
                "UseResult.unless",
                &[rig.labeled("reason", Some(("r", SPAN)))]
            ),
            None
        );
        assert_eq!(
            rig.unless_span("UseResult.unless", &[rig.param_defined(None)]),
            None
        );
    }

    /// A bare reference argument keeps the literal path silent (the value
    /// face is absent) and surfaces as the `Reference` face: the written
    /// name and span — the official `toString` fallback's message face;
    /// the gate value stays with the caller's const environment.
    #[test]
    fn unless_reference_face_carries_the_written_name() {
        let rig = Rig::new(&[Import::named("package:meta/meta.dart", "meta")], &[]);
        assert_eq!(
            rig.unless_span("UseResult.unless", &[rig.referenced("s", SPAN)]),
            Some(SPAN)
        );
        assert_eq!(
            rig.unless_face("UseResult.unless", &[rig.referenced("s", SPAN)]),
            Some(UnlessFace::Reference {
                name: rig.strings.intern("s"),
                span: SPAN,
            })
        );
        assert_eq!(
            rig.unless_face("UseResult.unless", &[rig.param_defined(Some(("p", SPAN)))]),
            Some(UnlessFace::Literal {
                value: rig.strings.intern("p"),
                span: SPAN,
            })
        );
    }

    // —— the dart:core deprecation tag on the ENGINE graph (#3321) ——————

    /// The engine's graph posture, which [`Rig`] does not model: a
    /// `dart:core` node carrying its public face, plus the implicit import
    /// edge every live library gets. A bare `@deprecated` therefore
    /// RESOLVES into dart:core instead of missing the namespace — the shape
    /// the tag test only ever saw as a miss, which silenced the whole
    /// `deprecated_member_use` face on the engine graph.
    struct CoreRig {
        strings: StringTable,
        graph: LibraryGraph,
        namespaces: Namespaces,
        lib: LibraryId,
    }

    impl CoreRig {
        /// `core`: the `dart:core` node's declared face (`None` builds the
        /// rig posture with no core node at all). `locals`: the analyzed
        /// library's own top-level declarations. `declared`: extra libraries
        /// `(uri, names)` the imports can reach. `imports`: explicit edges,
        /// appended after the implicit core one when `core` is `Some` and
        /// `suppress_implicit_core` is false — the engine drops the implicit
        /// edge for a library that writes its own `dart:core` import.
        fn build(
            core: Option<&[&'static str]>,
            locals: &[&'static str],
            declared: &[(&'static str, &'static [&'static str])],
            imports: &[Import],
            suppress_implicit_core: bool,
        ) -> Self {
            let mut strings = StringTable::new();
            let mut files = FileTable::new();
            // Pre-intern every spelling while `strings` is still ours alone
            // (the builder holds a shared borrow through `finish`).
            let face: Vec<StringId> = core
                .unwrap_or_default()
                .iter()
                .map(|n| strings.intern(n))
                .collect();
            let locals: Vec<StringId> = locals.iter().map(|n| strings.intern(n)).collect();
            let declared: Vec<(StringId, Vec<StringId>)> = declared
                .iter()
                .map(|(uri, names)| {
                    (
                        strings.intern(uri),
                        names.iter().map(|n| strings.intern(n)).collect(),
                    )
                })
                .collect();
            let edges: Vec<(&'static str, Option<StringId>, Combinators)> = imports
                .iter()
                .map(|import| {
                    (
                        import.uri,
                        import.prefix.map(|p| strings.intern(p)),
                        import.combinators(&mut strings),
                    )
                })
                .collect();
            let mut builder = LibraryGraphBuilder::new(&strings);
            let core_node = core.map(|_| builder.intern("dart:core"));
            if let Some(node) = core_node {
                // The engine registers the SDK node's names-only face file as
                // a unit: that `file -> library` row is what the tag arm's
                // dart:core identity test reads.
                let file = files.intern(Path::new("<dart:core>"));
                builder.face_unit(node, file);
                for name in face {
                    builder.declare(node, name, file);
                }
            }
            let lib = builder.intern("package:app/app.dart");
            let app_file = files.intern(Path::new("lib/app.dart"));
            for local in locals {
                builder.declare(lib, local, app_file);
            }
            for (uri, names) in declared {
                let uri_text = strings.resolve(uri);
                let node = builder.intern(uri_text);
                let file = files.intern(Path::new(uri_text));
                for name in names {
                    builder.declare(node, name, file);
                }
            }
            if let Some(core) = core_node {
                if !suppress_implicit_core {
                    builder.import(
                        lib,
                        ImportEdge {
                            target: Some(core),
                            prefix: None,
                            deferred: false,
                            non_library: false,
                            combinators: Combinators::unfiltered(),
                        },
                    );
                }
            }
            for (uri, prefix, combinators) in edges {
                let target = builder.intern(uri);
                builder.import(
                    lib,
                    ImportEdge {
                        target: Some(target),
                        prefix,
                        deferred: false,
                        non_library: false,
                        combinators,
                    },
                );
            }
            let graph = builder.finish();
            let namespaces = Namespaces::build(&graph);
            Self {
                strings,
                graph,
                namespaces,
                lib,
            }
        }

        fn tag(&self, spelling: &str) -> bool {
            is_core_deprecated_spelling(
                spelling,
                &self.strings,
                &self.graph,
                &self.namespaces,
                self.lib,
            )
        }
    }

    /// The engine graph resolves a bare `@deprecated` / `@Deprecated` INTO
    /// dart:core through the implicit import edge — the arm that used to be
    /// unreachable (`Lookup::NotFound` only), leaving every
    /// `deprecated_member_use` row unemitted.
    #[test]
    fn bare_deprecation_tag_resolves_into_the_core_node() {
        let rig = CoreRig::build(Some(&["deprecated", "Deprecated"]), &[], &[], &[], false);
        assert!(rig.tag("deprecated"), "const tag through the implicit edge");
        assert!(rig.tag("Deprecated"), "class tag through the implicit edge");
        assert!(
            !rig.tag("experimental"),
            "an untagged spelling stays silent"
        );
    }

    /// The rig posture (no `dart:core` node): the implicit import is not
    /// modelled, so the bare spelling still reads as the tag. Keeps the
    /// hand-built-rig pins of the member-use faces green.
    #[test]
    fn bare_deprecation_tag_survives_a_graph_without_dart_core() {
        let rig = CoreRig::build(None, &[], &[], &[], false);
        assert!(rig.tag("deprecated"));
    }

    /// A local declaration shadows the tag — the annotation resolves to it,
    /// never to dart:core (`element/scope.dart:419-433`).
    #[test]
    fn local_deprecation_spelling_shadows_the_core_tag() {
        let rig = CoreRig::build(
            Some(&["deprecated", "Deprecated"]),
            &["deprecated"],
            &[],
            &[],
            false,
        );
        assert!(
            !rig.tag("deprecated"),
            "the local wins over the implicit import"
        );
        assert!(rig.tag("Deprecated"), "the other spelling is untouched");
    }

    /// A same-spelled declaration in an imported library is not the tag.
    #[test]
    fn imported_same_spelling_is_not_the_tag() {
        let rig = CoreRig::build(
            Some(&["deprecated", "Deprecated"]),
            &[],
            &[("package:other/other.dart", &["deprecated"])],
            &[Import::unprefixed("package:other/other.dart")],
            false,
        );
        assert!(!rig.tag("deprecated"));
    }

    /// An explicit `dart:core` import replaces the implicit edge; a `show`
    /// that filters the tag out leaves it unresolvable, and an
    /// unresolvable annotation is no tag.
    #[test]
    fn a_filtered_explicit_core_import_hides_the_tag() {
        let rig = CoreRig::build(
            Some(&["deprecated", "Deprecated", "String"]),
            &[],
            &[],
            &[Import {
                uri: "dart:core",
                name: None,
                prefix: None,
                show: Some(&["String"]),
                hide: None,
            }],
            true,
        );
        assert!(!rig.tag("deprecated"));
    }

    /// The prefixed face: `@core.deprecated` is the tag only under an
    /// `import 'dart:core' as core;`.
    #[test]
    fn prefixed_deprecation_tag_follows_the_dart_core_import() {
        let rig = CoreRig::build(
            Some(&["deprecated", "Deprecated"]),
            &[],
            &[],
            &[Import::prefixed("dart:core", "core")],
            false,
        );
        assert!(rig.tag("core.deprecated"));
        let other = CoreRig::build(
            Some(&["deprecated", "Deprecated"]),
            &[],
            &[("package:other/other.dart", &["deprecated"])],
            &[Import::prefixed("package:other/other.dart", "core")],
            false,
        );
        assert!(!other.tag("core.deprecated"));
    }

    /// The message face: a positional / `message:` argument is normalized
    /// the way the oracle presents it (`normalizeDeprecationMessage`), and a
    /// bare `@deprecated` (no args) carries none.
    #[test]
    fn deprecation_message_normalizes_like_the_oracle() {
        let mut strings = StringTable::new();
        let args = |text: &'static str| Some(strings.intern(text));
        assert_eq!(deprecation_message(None, &strings), None);
        assert_eq!(deprecation_message(args("()"), &strings), None);
        assert_eq!(deprecation_message(args("('.')"), &strings), None);
        assert_eq!(
            deprecation_message(args("('Use x')"), &strings),
            Some("Use x.".to_owned())
        );
        assert_eq!(
            deprecation_message(args("('Use x?')"), &strings),
            Some("Use x?".to_owned())
        );
        assert_eq!(
            deprecation_message(args("(message: 'Use x')"), &strings),
            Some("Use x.".to_owned())
        );
        assert_eq!(
            deprecation_message(args("(expires: '2024')"), &strings),
            Some("2024.".to_owned())
        );
    }
}
