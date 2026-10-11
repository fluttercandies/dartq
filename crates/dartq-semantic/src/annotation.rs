//! package:meta getter recognition (#705): whether a declaration's
//! annotation is one of package:meta's top-level-getter annotations — the
//! `@sealed` face (W6's `mixin_on_sealed_class` / `subtype_of_sealed_class`
//! producers) and the `@literal` face (W7's
//! `non_const_call_to_literal_constructor` producer) — so those lanes are
//! pure verdict logic over the captured [`crate::model::Container::annotations`].
//!
//! # The official decision, mirrored
//!
//! The oracle resolves the annotation through the ordinary scope chain and
//! tests the resolved element (`_isPackageMetaGetter` /
//! `_isTopGetter`, analyzer `element.dart:2100-2130`): a GetterElement with
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
//! 3. the target library is meta — its canonical uri is
//!    `package:meta/…`, the uri-space stand-in for the element's library
//!    name (the corpus's vendored stand-in resolves `package:meta/meta.dart`
//!    and carries `library meta;` for the oracle's own name test);
//! 4. the import's combinators allow the getter name (a `show`/`hide`
//!    filters the oracle's namespace build too).
//!
//! Known limit (under-report direction, no fixture face): a getter
//! re-exported through a barrel library is not recognized — the import
//! target's uri must itself be `package:meta/…`, not merely transitively
//! export it.

use dartq_resolution::{LibraryGraph, LibraryId, LocalLookup};
use dartq_source::{StringId, StringTable};

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
    let spelling = strings.resolve(annotation.name);
    match spelling.split_once('.') {
        None => {
            spelling == getter
                && matches!(graph.local(lib, annotation.name), LocalLookup::Absent)
                && imports_meta(graph, lib, None, annotation.name)
        }
        Some((prefix, rest)) => {
            rest == getter
                && imports_meta(
                    graph,
                    lib,
                    Some(strings.intern(prefix)),
                    strings.intern(getter),
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
                .is_some_and(|target| is_meta_uri(graph.uri(target)))
            && edge.combinators.allows(getter_id)
    })
}

/// The canonical-uri form of the meta test: the official check reads the
/// resolved element's library name (`_metaLibName`, `element.dart:1725`);
/// the graph keys libraries by canonical uri, and every library of the
/// meta package spells `package:meta/…`.
fn is_meta_uri(uri: &str) -> bool {
    uri.starts_with("package:meta/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use dartq_resolution::{CombinatorClause, Combinators, ImportEdge, LibraryGraphBuilder};
    use dartq_source::{FileTable, SourceRange};
    use std::path::Path;

    /// The shared rig: an analyzed library plus the imports and local
    /// declarations a pin needs, mirroring the engine's graph build
    /// (`package:` canonical uris, one interned node per library).
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
            let locals: Vec<StringId> = locals.iter().map(|n| strings.intern(n)).collect();
            let mut builder = LibraryGraphBuilder::new(&strings);
            let lib = builder.intern("package:app/app.dart");
            for local in locals {
                let file = files.intern(Path::new("lib/app.dart"));
                builder.declare(lib, local, file);
            }
            for (uri, prefix, combinators) in edges {
                let target = builder.intern(uri);
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
                },
                getter,
                &self.strings,
                &self.graph,
                self.lib,
            )
        }
    }

    struct Import {
        uri: &'static str,
        prefix: Option<&'static str>,
        show: Option<&'static [&'static str]>,
    }

    impl Import {
        fn unprefixed(uri: &'static str) -> Self {
            Self {
                uri,
                prefix: None,
                show: None,
            }
        }

        fn prefixed(uri: &'static str, prefix: &'static str) -> Self {
            Self {
                uri,
                prefix: Some(prefix),
                show: None,
            }
        }

        fn combinators(&self, strings: &mut StringTable) -> Combinators {
            match self.show {
                None => Combinators::unfiltered(),
                Some(names) => Combinators::new(
                    [CombinatorClause::Show(
                        names.iter().map(|n| strings.intern(n)).collect(),
                    )],
                    strings,
                ),
            }
        }
    }

    /// A bare `@sealed` under an unprefixed `package:meta/meta.dart`
    /// import recognizes — the corpus stand-in's exact face (the vendored
    /// pkg/meta resolves the same canonical uri). A different getter name
    /// on the same spelling does not.
    #[test]
    fn bare_sealed_under_meta_import_recognizes() {
        let rig = Rig::new(&[Import::unprefixed("package:meta/meta.dart")], &[]);
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
    /// `sealed`: the uri test rejects it — the getter identity is the
    /// library, not the name.
    #[test]
    fn same_named_getter_outside_meta_does_not_recognize() {
        let rig = Rig::new(&[Import::unprefixed("package:other/other.dart")], &[]);
        assert!(!rig.recognizes("sealed", "sealed"));
    }

    /// A local top-level declaration shadows the unprefixed import: the
    /// annotation resolves locally, never to meta.
    #[test]
    fn local_shadow_suppresses_bare_recognition() {
        let rig = Rig::new(&[Import::unprefixed("package:meta/meta.dart")], &["sealed"]);
        assert!(!rig.recognizes("sealed", "sealed"));
    }

    /// A prefixed import recognizes through its prefix (`@meta.sealed`
    /// under `import ... as meta;`); a bare spelling never crosses a
    /// prefixed import, and the wrong prefix leaves the getter
    /// unresolved.
    #[test]
    fn prefixed_import_recognizes_by_prefix() {
        let rig = Rig::new(&[Import::prefixed("package:meta/meta.dart", "meta")], &[]);
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
                prefix: None,
                show: Some(&["literal"]),
            }],
            &[],
        );
        assert!(!rig.recognizes("sealed", "sealed"));
        assert!(rig.recognizes("literal", "literal"));
    }
}
