//! The best-practices access face: the `_checkForOtherInvalidAccess`
//! template leg (upstream Dart SDK source,
//! pkg/analyzer/lib/src/error/best_practices_verifier.dart:1824-1948) —
//! a member annotated `@visibleForTemplate` (package:angular_meta)
//! referenced from another library reports
//! `invalid_use_of_visible_for_template_member` at the member-name token.
//!
//! # The official decision, mirrored
//!
//! `verify` bails first on a same-library element (`_inCurrentLibrary`,
//! :1673-1675 — `element.library == _library`, any unit of the library).
//! The template leg then applies `_isVisibleForTemplateApplied`
//! (:1989-1996): the member — or its declaring container, through
//! `_hasVisibleForTemplate`'s enclosing cascade (:1981-2004) — resolves
//! the `visibleForTemplate` top-level getter of a library declared
//! `library angular.meta;` (`_isTopGetter`, element.dart:2014-2028;
//! `_angularMetaLibName`, element.dart:1677), and no
//! `visibleOutsideTemplate` rides it (`_hasVisibleOutsideTemplate`'s
//! cascade folds into the same two probes at name level). An access
//! from a template source is exempt (`_inTemplateSource`,
//! :1602/:1615-1619 — the library's defining file path carries
//! `.template`; mirrored over the graph's canonical URI, whose tail
//! preserves the `.template.dart` file name a package path carries), as
//! is an export-combinator reference (`_inExportDirective`, :2006-2008 —
//! unreachable in this funnel: export combinators never route through
//! member rows). The report anchors the member-name token with the
//! member name and the DECLARING library's URI (:1940-1946,
//! `definingClass.library!.uri`).
//!
//! Wired at [`Engine::member_defined`]'s read arm — the single funnel
//! every qualified resolved instance-member access (plain and `super`)
//! passes. Writes resolve the setter spelling upstream
//! (`writeOrReadElement`), and top-level declarations ride the
//! bare-reference funnel, not this one — both stay under-reports.

use super::*;
use crate::model::Annotation;
use dartq_resolution::LocalLookup;

impl<'a> Engine<'a> {
    /// The template leg of the official invalid-access family, run at a
    /// resolved read of `row`'s member: `entry` is the declaration the
    /// walk landed on and `landing` its (file, container) — the same
    /// snapshot [`Engine::member_defined`]'s read arm already took.
    pub(crate) fn check_visible_for_template_member(
        &mut self,
        row: &Row,
        entry: &MemberEntry,
        landing: Option<(FileId, StringId)>,
    ) {
        let Some((decl_file, decl_container)) = landing else {
            return;
        };
        // `_inCurrentLibrary` (verify, :1673-1675): a use inside the
        // declaring library — any of its units — is exempt before the
        // annotation consult.
        let Some(decl_lib) = self.graph.library_of_file(decl_file) else {
            return;
        };
        if decl_lib == self.lib {
            return;
        }
        // `_isVisibleForTemplateApplied` (:1989-1996): the annotation —
        // the member's own or the declaring container's (`_hasVisibleFor
        // Template`'s enclosing cascade, :1981-2004) — must resolve to the
        // angular-meta getter through the DECLARING library's import faces
        // (it is written in that library's source), and the mirror-image
        // `visibleOutsideTemplate` cascade (`_hasVisibleOutsideTemplate`,
        // :2000-2012) must be absent — one probe per getter over both
        // faces.
        let strings = self.strings;
        let graph = self.graph;
        let index = self.index;
        let carries = |getter: &str| {
            let member = entry.annotations.iter().any(|annotation| {
                is_angular_meta_getter(annotation, getter, strings, graph, decl_lib)
            });
            member
                || index
                    .get(decl_file)
                    .and_then(|elements| elements.container(decl_container))
                    .is_some_and(|decl| {
                        decl.annotations.iter().any(|annotation| {
                            is_angular_meta_getter(annotation, getter, strings, graph, decl_lib)
                        })
                    })
        };
        if !(carries("visibleForTemplate") && !carries("visibleOutsideTemplate")) {
            return;
        }
        // `_inTemplateSource` (:1602/:1615-1619): a template-source use is
        // exempt (the use-site library's `.template` path face).
        if graph.uri(self.lib).contains(".template") {
            return;
        }
        // The report (:1940-1946): the member name and the declaring
        // library's URI, at the member-name token (`row.range`, the
        // selector as written).
        let name = strings.resolve(row.name).to_owned();
        let uri = graph.uri(decl_lib).to_owned();
        self.emit(
            SemanticCode::InvalidUseOfVisibleForTemplateMember,
            row.range,
            &name,
            &uri,
        );
    }
}

/// Whether `annotation` resolves, through `lib`'s import faces, to the
/// top-level getter `getter` of a library declared
/// `library angular.meta;` — the official angular-meta face
/// (`_isTopGetter(libraryName: _angularMetaLibName, …)`,
/// element.dart:2014-2028 and :1677), the name-level twin of
/// [`crate::annotation::is_meta_getter`]'s meta face: the spelled name
/// (bare, or the segment after a prefix), a local-shadow gate on the
/// bare path, and an import edge whose target declares the name and
/// whose combinators allow the getter.
fn is_angular_meta_getter(
    annotation: &Annotation,
    getter: &str,
    strings: &StringTable,
    graph: &LibraryGraph,
    lib: LibraryId,
) -> bool {
    let spelling = strings.resolve(annotation.name);
    let getter_id = strings.intern(getter);
    let resolves = |prefix: Option<StringId>| {
        graph.import_edges(lib).iter().any(|edge| {
            edge.prefix == prefix
                && edge
                    .target
                    .is_some_and(|target| graph.name(target) == "angular.meta")
                && edge.combinators.allows(getter_id)
        })
    };
    match spelling.split_once('.') {
        None => {
            spelling == getter
                && matches!(graph.local(lib, getter_id), LocalLookup::Absent)
                && resolves(None)
        }
        Some((prefix, name)) => name == getter && resolves(Some(strings.intern(prefix))),
    }
}
