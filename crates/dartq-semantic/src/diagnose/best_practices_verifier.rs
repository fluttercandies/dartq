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

/// The #2910 pins (the member-use deprecation/experimental faces). Kept as
/// a standalone `#[path]` submodule so the shared `tests` rig and this
/// file's own module stay untouched by the registration.
#[cfg(test)]
#[path = "ticket_2910_tests.rs"]
mod ticket_2910_tests;

/// The #2924 pins (the top-level / parameter / extension / named-
/// constructor / library-frontier faces of the same reporter).
#[cfg(test)]
#[path = "ticket_2924_tests.rs"]
mod ticket_2924_tests;

/// The #2939 pins (the positional-parameter arm and the constructor
/// `displayName` join).
#[cfg(test)]
#[path = "ticket_2939_tests.rs"]
mod ticket_2939_tests;

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

// —— the member-use deprecation/experimental faces (#2910) —————————————————
//
// The oracle's `deprecated_member_use` / `experimental_member_use` warnings
// (upstream Dart SDK source, pkg/analyzer/lib/src/error/
// element_usage_detector.dart, deprecated_member_use_verifier.dart,
// experimental_member_use_verifier.dart, driven from
// best_practices_verifier.dart's `ElementUsageFrontierDetector`): every
// resolved reference to a declaration whose metadata carries the dart:core
// `@deprecated`/`@Deprecated` tag or package:meta's `@experimental`
// reports at the reference. Two suppressions mirror the oracle exactly:
// `isInSamePackage` (both reporters bail on a same-package declaration) and
// the frontier's sticky per-tag stacks (a use inside an enclosing
// declaration carrying the same tag never reports again).
//
// This pass rides the resolution products (`member_refs` edges, bare
// `refs`, `type_uses`) after pass 2 — the emission funnel itself lives in
// row_resolution.rs (a sibling's file), so the tags are checked against the
// resolved targets here rather than at each resolve arm.

/// The `deprecated_member_use` code (both message shapes share the printed
/// name; the with-message row's unique name is
/// `deprecated_member_use_with_message` — pinned registry
/// `diagnostic.g.dart:3858/:3876`).
const DEPRECATED_MEMBER_USE: CodeInfo = CodeInfo::new(
    "deprecated_member_use",
    "DEPRECATED_MEMBER_USE",
    DiagnosticType::Hint,
    true,
);
/// The `experimental_member_use` code (a `STATIC_WARNING` with no
/// correction message — `diagnostic.g.dart:5266`).
const EXPERIMENTAL_MEMBER_USE: CodeInfo = CodeInfo::new(
    "experimental_member_use",
    "EXPERIMENTAL_MEMBER_USE",
    DiagnosticType::StaticWarning,
    false,
);
const DEPRECATED_CORRECTION: &str =
    "Try replacing the use of the deprecated member with the replacement.";

/// One tag's frontier: the body extents of the analyzed unit's own
/// declarations carrying the tag, offset-sorted.
#[derive(Default)]
struct Frontier {
    extents: Vec<(u32, u32)>,
}

impl Frontier {
    fn covers(&self, offset: u32) -> bool {
        self.extents
            .iter()
            .any(|&(start, end)| start <= offset && offset < end)
    }
}

impl Engine<'_> {
    /// Both faces over every resolved reference the unit carries. Runs
    /// after pass 2 emptied the row stack, so `member_refs`/`refs`/
    /// `type_uses` are final.
    pub(crate) fn report_member_use_warnings(&mut self) {
        let frontier = self.build_frontier();
        // One emission per (use offset, tag): the bare-reference and
        // type-use publications overlap by design (a creation head rides
        // both), and both faces answer for the same declaration.
        let mut reported: FxHashSet<(u32, u8)> = FxHashSet::default();
        let member_refs = std::mem::take(&mut self.report.member_refs);
        for edge in &member_refs {
            let spelled = self.strings.resolve(edge.target_name);
            let display = spelled.strip_suffix('=').unwrap_or(spelled).to_owned();
            self.check_target_annotations(
                edge.range,
                &display,
                edge.target_file,
                edge.target_container,
                Some(edge.target_name),
                &frontier,
                &mut reported,
            );
        }
        self.report.member_refs = member_refs;
        let refs = std::mem::take(&mut self.refs);
        for reference in &refs {
            let RefTarget::Decl { file } = reference.target else {
                continue;
            };
            let display = self.strings.resolve(reference.name).to_owned();
            self.check_target_annotations(
                reference.range,
                &display,
                file,
                reference.name,
                None,
                &frontier,
                &mut reported,
            );
        }
        self.refs = refs;
        let type_uses = std::mem::take(&mut self.type_uses);
        for (range, name) in &type_uses {
            let dartq_resolution::Lookup::Found(dartq_resolution::NamespaceEntry::Decl(file)) =
                self.namespaces.lookup(self.graph, self.lib, *name)
            else {
                continue;
            };
            let display = self.strings.resolve(*name).to_owned();
            self.check_target_annotations(
                *range,
                &display,
                file,
                *name,
                None,
                &frontier,
                &mut reported,
            );
        }
        self.type_uses = type_uses;
    }

    /// The tag faces of one resolved use: the target declaration's model
    /// row (a container's, an extension member's, or — with `member` — a
    /// container member's), its annotations, and each tag's report.
    fn check_target_annotations(
        &mut self,
        range: SourceRange,
        display: &str,
        target_file: FileId,
        target_container: StringId,
        member: Option<StringId>,
        frontier: &Frontiers,
        reported: &mut FxHashSet<(u32, u8)>,
    ) {
        let Some(elements) = self.index.get(target_file) else {
            return;
        };
        // The constructor arm's rendered spelling: upstream reports a
        // `ConstructorElement` use under its `displayName`
        // (element.dart:838-847 — `$className.$name` for a named
        // constructor, the bare `className` for the `new`/unnamed face),
        // while the member walks above keep the selector alone
        // (`MethodElement.displayName` is the member name). Computed beside
        // the annotations so the fallback that wins supplies both.
        let mut ctor_display: Option<String> = None;
        let annotations: &[crate::model::Annotation] = match member {
            Some(name) => elements
                .container(target_container)
                .and_then(|decl| decl.members.get(&name))
                .map(|entry| entry.annotations.as_ref())
                .or_else(|| {
                    elements.extension_faces().iter().find_map(|face| {
                        (face.name == Some(target_container)
                            || (face.name.is_none()
                                && target_container == StringId::NONE))
                            .then(|| face.members.get(&name))
                            .flatten()
                            .map(|entry| entry.annotations.as_ref())
                    })
                })
                .or_else(|| {
                    // A constructor carries no MemberEntry row — `A.named`/
                    // `A.new`/`A` resolves to the container with the
                    // selector as member. The model keys the unnamed face
                    // under the class name (`ctor_formals`'s own spelling),
                    // so `new` and the bare `A(` head both normalize to the
                    // container — and the `displayName` join renders that
                    // same normalization (element.dart:838-847: the
                    // class's spelling alone for the unnamed face,
                    // `Class.selector` for a named one).
                    let selector = self.strings.resolve(name);
                    let key = if selector == "new" {
                        target_container
                    } else {
                        name
                    };
                    let container_spelling = self.strings.resolve(target_container);
                    let rendered = if key == target_container {
                        container_spelling.to_owned()
                    } else {
                        format!("{container_spelling}.{selector}")
                    };
                    ctor_display = Some(rendered);
                    elements.ctor_annotations(target_container, key)
                })
                .unwrap_or(&[]),
            None => elements
                .container(target_container)
                .map(|decl| decl.annotations.as_ref())
                .or_else(|| {
                    // A named extension's use (`E(0).foo`, `E` in a type
                    // position) resolves to the extension face, not a
                    // container row (upstream `visitExtensionOverride`
                    // reads the extension element's own metadata).
                    elements.extension_faces().iter().find_map(|face| {
                        (face.name == Some(target_container))
                            .then(|| face.annotations.as_ref())
                    })
                })
                .or_else(|| {
                    // The top-level declaration face: an annotated
                    // function, variable, or typedef the container walk
                    // never sees.
                    elements
                        .decl(target_container)
                        .map(|decl| decl.annotations.as_ref())
                })
                .unwrap_or(&[]),
        };
        if annotations.is_empty() {
            return;
        }
        // `isInSamePackage` (:46 deprecated reporter, :28 experimental):
        // both reporters bail before reading the tag.
        if self.use_is_same_package(target_file) {
            return;
        }
        let strings = self.strings;
        let graph = self.graph;
        let namespaces = self.namespaces;
        let Some(decl_lib) = graph.library_of_file(target_file) else {
            return;
        };
        let display = ctor_display.as_deref().unwrap_or(display);
        for annotation in annotations {
            if crate::annotation::is_core_deprecated(
                annotation, strings, graph, namespaces, decl_lib,
            ) && !frontier.deprecated.covers(range.offset)
                && reported.insert((range.offset, 0))
            {
                self.report_deprecated_member_use(range, display, annotation);
            }
            if crate::annotation::is_meta_getter(
                annotation,
                "experimental",
                strings,
                graph,
                decl_lib,
            ) && !frontier.experimental.covers(range.offset)
                && reported.insert((range.offset, 1))
            {
                self.report_experimental_member_use(range, display);
            }
        }
    }

    /// The `deprecated_member_use` row: plain or with-message shape at the
    /// reference (the member-name / type-name token — the oracle's
    /// `SimpleIdentifier` anchor; assignment LHS rows carry the selector
    /// token already).
    fn report_deprecated_member_use(
        &mut self,
        range: SourceRange,
        display: &str,
        annotation: &crate::model::Annotation,
    ) {
        let message =
            match crate::annotation::deprecation_message(annotation.args_text, self.strings) {
                Some(tail) => format!("'{display}' is deprecated and shouldn't be used. {tail}"),
                None => format!("'{display}' is deprecated and shouldn't be used."),
            };
        self.report.usage.push(Diagnostic::new(
            DEPRECATED_MEMBER_USE.dtype.severity(),
            DEPRECATED_MEMBER_USE,
            message,
            self.file,
            range,
            Some(DEPRECATED_CORRECTION.to_owned()),
        ));
    }

    /// The `experimental_member_use` row.
    fn report_experimental_member_use(&mut self, range: SourceRange, display: &str) {
        self.report.usage.push(Diagnostic::new(
            EXPERIMENTAL_MEMBER_USE.dtype.severity(),
            EXPERIMENTAL_MEMBER_USE,
            format!("'{display}' is experimental and could be removed or changed at any time."),
            self.file,
            range,
            None,
        ));
    }

    /// The `deprecated_member_use` face's parameter arm (upstream
    /// `element_usage_detector.dart` `_visitParametersAndArguments`,
    /// :381-413): a call site's `name:`-labeled argument whose bound
    /// formal carries the tag reports at the LABEL — the
    /// NamedExpression's own anchor, not the callee's.
    ///
    /// `owner` is the collector's `formal_annotations` join spelling (a
    /// top-level `f`, a member `C.m`, a constructor `C.<key>`); `name` the
    /// label as written (the public formal spelling —
    /// `correspondingPublicName`, token_impl.dart:364).
    pub(crate) fn check_formal_arg_annotations(
        &mut self,
        range: SourceRange,
        name: StringId,
        target_file: FileId,
        owner: StringId,
    ) {
        let Some(elements) = self.index.get(target_file) else {
            return;
        };
        let Some(annotations) = elements.formal_annotations(owner, name) else {
            return;
        };
        // `isInSamePackage` — the same gate the member faces read.
        if self.use_is_same_package(target_file) {
            return;
        }
        let strings = self.strings;
        let graph = self.graph;
        let namespaces = self.namespaces;
        let Some(decl_lib) = graph.library_of_file(target_file) else {
            return;
        };
        // `_isLocalParameter` (:416-419): a use inside the parameter's own
        // executable is the declaration's own machinery — no report. The
        // owner key's bare member name (the text after the last `.` for a
        // `C.m` join, the whole key for a top-level `f`) names the
        // executable; a label inside its own body extent is exempt. The
        // frontier's per-tag stacks already cover the tag-carrying
        // declaration; this is the tag-free owner's own-parameter arm.
        let display = strings.resolve(name).to_owned();
        let owner_spelling = strings.resolve(owner);
        let owner_member = owner_spelling.rsplit('.').next().unwrap_or(owner_spelling);
        let in_own_executable = self.decl_sites.iter().any(|site| {
            strings.resolve(site.name) == owner_member
                && {
                    let extent = self.declaration_extent(site.range.offset);
                    extent.0 <= range.offset && range.offset < extent.1
                }
        });
        if in_own_executable {
            return;
        }
        // The positional arm's revisit frontier: a still-open argument
        // group's unlabeled arguments re-pair at every later `name:` label
        // (`check_positional_formal_annotations` drains the live level —
        // the arglist-close seam sits in a sibling lane's file), so the
        // same (argument offset, tag) can arrive more than once. Upstream's
        // single walk reports it once; the emitted rows already carry the
        // verdict, making the duplicate visit a no-op at name level. The
        // named-label arm is visit-once by construction — the same check
        // costs it nothing.
        for annotation in annotations {
            let emitted_dep = self.report.usage.iter().any(|row| {
                row.range.offset == range.offset && row.code.name == "deprecated_member_use"
            });
            if crate::annotation::is_core_deprecated(
                annotation, strings, graph, namespaces, decl_lib,
            ) && !emitted_dep
            {
                self.report_deprecated_member_use(range, &display, annotation);
            }
            let emitted_exp = self.report.usage.iter().any(|row| {
                row.range.offset == range.offset && row.code.name == "experimental_member_use"
            });
            if crate::annotation::is_meta_getter(
                annotation,
                "experimental",
                strings,
                graph,
                decl_lib,
            ) && !emitted_exp
            {
                self.report_experimental_member_use(range, &display);
            }
        }
    }

    /// `isInSamePackage`'s name-level mirror: the target library's
    /// canonical URI and this library's name the same package
    /// (`package:<name>/…` heads; a same-library target is trivially
    /// contained). A non-package URI on either side (a `dart:`-grade or
    /// file-less library) is never "the same package" — the oracle's
    /// workspace test answers false there too.
    fn use_is_same_package(&self, target_file: FileId) -> bool {
        let Some(target_lib) = self.graph.library_of_file(target_file) else {
            return false;
        };
        if target_lib == self.lib {
            return true;
        }
        match (
            package_of_uri(self.graph.uri(self.lib)),
            package_of_uri(self.graph.uri(target_lib)),
        ) {
            (Some(ours), Some(theirs)) => ours == theirs,
            _ => false,
        }
    }

    /// The analyzed unit's own tagged declarations, with their body
    /// extents — the frontier detector's per-tag stacks at name level
    /// (`ElementUsageFrontierDetector.pushElement`/`shouldCheck*ForIndex`:
    /// once a walk enters a tagged declaration, no further use of that
    /// tag's face reports inside it, at any nesting depth).
    fn build_frontier(&self) -> Frontiers {
        let mut frontiers = Frontiers::default();
        // `@deprecated library lib;` — upstream pushes the LIBRARY
        // element's tags onto the same sticky stacks as every
        // declaration's (`pushElement(_currentLibrary)`,
        // best_practices_verifier.dart:166), so a tagged directive
        // silences the tag over the whole unit.
        if let Some(elements) = self.index.get(self.file) {
            for annotation in elements.library_annotations() {
                if crate::annotation::is_core_deprecated(
                    annotation,
                    self.strings,
                    self.graph,
                    self.namespaces,
                    self.lib,
                ) {
                    frontiers.deprecated.extents.push((0, u32::MAX));
                }
                if crate::annotation::is_meta_getter(
                    annotation,
                    "experimental",
                    self.strings,
                    self.graph,
                    self.lib,
                ) {
                    frontiers.experimental.extents.push((0, u32::MAX));
                }
            }
        }
        for site in &self.decl_sites {
            let mut deprecated = false;
            let mut experimental = false;
            for name in site.annotations.iter() {
                let spelling = self.strings.resolve(*name);
                if crate::annotation::is_core_deprecated_spelling(
                    spelling,
                    self.strings,
                    self.graph,
                    self.namespaces,
                    self.lib,
                ) {
                    deprecated = true;
                }
                if crate::annotation::is_meta_getter(
                    // The name-level scratch face: the meta getter test
                    // reads the spelling only.
                    &crate::model::Annotation {
                        name: *name,
                        range: site.range,
                        args_text: None,
                    },
                    "experimental",
                    self.strings,
                    self.graph,
                    self.lib,
                ) {
                    experimental = true;
                }
            }
            if deprecated || experimental {
                let extent = self.declaration_extent(site.range.offset);
                if deprecated {
                    frontiers.deprecated.extents.push(extent);
                }
                if experimental {
                    frontiers.experimental.extents.push(extent);
                }
            }
        }
        frontiers
    }

    /// The body span a declaration opens at its name token: the first
    /// top-level `{` (a braced body — class, mixin, method, and the like)
    /// matched to its closer, or the terminating `;` (a variable,
    /// typedef, or signature-only member). Scans to end of file when
    /// neither closes — recovery residue keeps the widest sound interval.
    fn declaration_extent(&self, name_offset: u32) -> (u32, u32) {
        let tokens = self.tokens.as_slice();
        let start = tokens.partition_point(|t| t.utf16_offset < name_offset);
        let mut group_depth = 0i32;
        let mut body_depth: Option<u32> = None;
        for token in &tokens[start..] {
            match token.kind {
                dartq_scanner::TokenKind::OpenParen | dartq_scanner::TokenKind::OpenSquare => {
                    group_depth += 1
                }
                dartq_scanner::TokenKind::CloseParen | dartq_scanner::TokenKind::CloseSquare => {
                    group_depth -= 1
                }
                dartq_scanner::TokenKind::OpenCurly
                | dartq_scanner::TokenKind::StringInterpolationExpression => {
                    if group_depth <= 0 {
                        *body_depth.get_or_insert(0) += 1;
                    }
                }
                dartq_scanner::TokenKind::CloseCurly => {
                    if let Some(depth) = body_depth.as_mut() {
                        *depth -= 1;
                        if *depth == 0 {
                            return (name_offset, token.utf16_offset + 1);
                        }
                    }
                }
                dartq_scanner::TokenKind::Semicolon if body_depth.is_none() && group_depth <= 0 => {
                    return (name_offset, token.utf16_offset + 1);
                }
                _ => {}
            }
        }
        (name_offset, u32::MAX)
    }
}

/// Both tags' frontiers.
#[derive(Default)]
struct Frontiers {
    deprecated: Frontier,
    experimental: Frontier,
}

/// The package name a canonical URI names (`package:<name>/…`), `None` for
/// every other scheme.
fn package_of_uri(uri: &str) -> Option<&str> {
    uri.strip_prefix("package:")
        .and_then(|rest| rest.split('/').next())
        .filter(|name| !name.is_empty())
}
