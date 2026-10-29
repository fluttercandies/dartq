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

use dartq_resolution::{LibraryGraph, LibraryId, LocalLookup};
use dartq_source::{SourceRange, StringId, StringTable};

use crate::model::Annotation;

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
}
