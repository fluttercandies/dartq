//! The best-practices access face: `_checkForOtherInvalidAccess`
//! (upstream Dart SDK source,
//! pkg/analyzer/lib/src/error/best_practices_verifier.dart:1824-1905) —
//! the `@protected` / `@visibleForTemplate` / `@visibleForTesting` /
//! `@visibleForOverriding` legs, plus the `type_annotation_deferred_class`
//! arm (`error_verifier.dart` `_checkForTypeAnnotationDeferredClass`
//! :7132-7139).
//!
//! # The official decision, mirrored
//!
//! `verify` bails first on a same-library element (`_inCurrentLibrary`,
//! :1673-1675 — `element.library == _library`, any unit of the library).
//! `_checkForOtherInvalidAccess` then applies the four legs in a fixed
//! order where the first three are *gates that return before any report*:
//!
//! * `@protected` (:1826-1832): the access sits inside a container whose
//!   type has the declaring class among its supertypes
//!   (`_hasTypeOrSuperType` :1910-1916) — every leg is exempt;
//! * `@visibleForTemplate` (:1834-1839): `_isVisibleForTemplateApplied`
//!   (:1989-1996 — the element's or its declaring container's
//!   `visibleForTemplate` angular-meta getter, no `visibleOutsideTemplate`
//!   riding it) against a template source (`_inTemplateSource`,
//!   :1602/:1615-1619 — the library's file path carries `.template`,
//!   mirrored over the graph's canonical URI);
//! * `@visibleForTesting` (:1841-1846): the analyzed unit is in a test
//!   directory (`_inTestDirectory`, :1617-1621);
//!
//! and the tail (:1850-1904) reports one row per annotation still
//! standing, in the same order. `@visibleForOverriding` adds one
//! exemption first (:1850-1863): a `super.x` access inside the very
//! member that overrides `x` is the sanctioned use.
//!
//! Package:meta getters (`protected`, `visibleForTesting`,
//! `visibleForOverriding` — `_isPackageMetaGetter` element.dart:2010-2012
//! over `_metaLibName` :1661) read the element's OWN metadata only: a
//! class-level annotation never cascades to its members. The
//! `@visibleForTemplate` leg is the angular-meta face (`_isTopGetter`
//! element.dart:2014-2028 over `_angularMetaLibName` :1677), whose
//! `_hasVisibleForTemplate` carries the enclosing cascade (:1930-1946).
//! The `_inExportDirective` exemption (:2006-2008) is unreachable in
//! these funnels — export combinators never route through member, bare
//! reference, or type-reference rows.
//!
//! # Where it is wired
//!
//! * [`Engine::member_defined`]'s read arm (the single funnel every
//!   qualified resolved instance-member access — plain and `super` —
//!   passes), via [`Engine::check_visible_for_template_member`];
//! * the post-pass publications [`Engine::report_member_use_warnings`]
//!   walks: bare references (top-level declarations and containers),
//!   type references (a container, or the constructor a creation head
//!   names), and the setter-write edges the read arm never sees.
//!
//! Setter writes through the member funnel's write arm, named-argument
//! labels, object-pattern fields, and the binary-operator face
//! (`verifyBinary`, :1690-1714) stay under-reports this round.

use super::*;
use crate::model::Annotation;
use dartq_resolution::LocalLookup;
use dartq_scanner::{Keyword, TokenKind};

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

/// The #2993 `invalid_annotation_target` face (the annotation-target
/// verifier, annotation_verifier.dart `_checkKinds`).
#[path = "annotation.rs"]
mod annotation;

/// The #2993 `unnecessary_final` / `missing_required_param` faces (the
/// parameter-shape members of the best-practices family).
#[path = "member_face.rs"]
mod member_face;

/// The #2993 pins (the four new faces' oracle-verbatim rows).
#[cfg(test)]
#[path = "ticket_2993_tests.rs"]
mod ticket_2993_tests;

/// The #3353 `deprecated_*` functionality faces: the declaration-clause
/// (`deprecated_extend`/`implement`/`mixin`/`subclass`), the creation
/// (`deprecated_instantiate`), and the omission (`deprecated_optional`)
/// verifier, plus the pre-3.0 `Function` arms.
#[path = "deprecated_functionality_verifier.rs"]
mod deprecated_functionality_verifier;

/// The #3353 pins (the `deprecated_*` functionality faces at the oracle's own
/// anchors).
#[cfg(test)]
#[path = "ticket_3353_tests.rs"]
mod ticket_3353_tests;

/// The `invalid_use_of_protected_member` code (a `WarningCode`
/// `STATIC_WARNING`; upstream Dart SDK source
/// pkg/analyzer/lib/src/diagnostic/diagnostic.g.dart, the registry row
/// `invalid_use_of_protected_member`).
const INVALID_USE_OF_PROTECTED_MEMBER: CodeInfo = CodeInfo::new(
    "invalid_use_of_protected_member",
    "INVALID_USE_OF_PROTECTED_MEMBER",
    DiagnosticType::StaticWarning,
    true,
);
/// The `invalid_use_of_visible_for_testing_member` code (a `WarningCode`
/// `STATIC_WARNING`).
const INVALID_USE_OF_VISIBLE_FOR_TESTING_MEMBER: CodeInfo = CodeInfo::new(
    "invalid_use_of_visible_for_testing_member",
    "INVALID_USE_OF_VISIBLE_FOR_TESTING_MEMBER",
    DiagnosticType::StaticWarning,
    true,
);
/// The `invalid_use_of_visible_for_overriding_member` code (a
/// `WarningCode` `STATIC_WARNING`).
const INVALID_USE_OF_VISIBLE_FOR_OVERRIDING_MEMBER: CodeInfo = CodeInfo::new(
    "invalid_use_of_visible_for_overriding_member",
    "INVALID_USE_OF_VISIBLE_FOR_OVERRIDING_MEMBER",
    DiagnosticType::StaticWarning,
    true,
);
/// The `type_annotation_deferred_class` code (a `CompileTimeErrorCode`
/// `COMPILE_TIME_ERROR` with a correction message,
/// `error_verifier.dart:7132-7139`).
const TYPE_ANNOTATION_DEFERRED_CLASS: CodeInfo = CodeInfo::new(
    "type_annotation_deferred_class",
    "TYPE_ANNOTATION_DEFERRED_CLASS",
    DiagnosticType::CompileTimeError,
    true,
);
const DEFERRED_CLASS_CORRECTION: &str =
    "Try using a different type, or changing the import to not be deferred.";

/// The four legs' verdicts for one resolved reference: which of the
/// access annotations the referenced element carries.
#[derive(Clone, Copy, Default)]
struct AccessLegs {
    protected: bool,
    template: bool,
    testing: bool,
    overriding: bool,
}

impl AccessLegs {
    fn any(self) -> bool {
        self.protected || self.template || self.testing || self.overriding
    }
}

impl<'a> Engine<'a> {
    /// The official invalid-access family, run at a resolved read of
    /// `row`'s member: `entry` is the declaration the walk landed on and
    /// `landing` its (file, container) — the same snapshot
    /// [`Engine::member_defined`]'s read arm already took. The historical
    /// name survives because the call site sits in a sibling lane's file;
    /// the body runs the whole `_checkForOtherInvalidAccess` chain.
    pub(crate) fn check_visible_for_template_member(
        &mut self,
        row: &Row,
        entry: &MemberEntry,
        landing: Option<(FileId, StringId)>,
    ) {
        let Some((decl_file, decl_container)) = landing else {
            return;
        };
        let index = self.index;
        let graph = self.graph;
        let Some(decl_lib) = graph.library_of_file(decl_file) else {
            return;
        };
        let container_annos: &[Annotation] = index
            .get(decl_file)
            .and_then(|elements| elements.container(decl_container))
            .map_or(&[], |decl| decl.annotations.as_ref());
        // The legs are computed here, off the walk's own entry: the entry's
        // borrow is tied to the local walk binding, so nothing of it may
        // ride into the emission path below.
        let legs = self.access_legs(entry.annotations.as_ref(), container_annos, decl_lib);
        let strings = self.strings;
        self.check_invalid_access(
            row.range,
            strings.resolve(row.name),
            decl_file,
            decl_container,
            legs,
            matches!(
                &row.shape,
                RowShape::Member {
                    recv: Receiver::Super { .. }
                }
            ),
        );
    }

    /// One resolved reference's annotations → legs → the
    /// `_checkForOtherInvalidAccess` chain (the post-pass funnels, whose
    /// annotation rows live in the engine's own index).
    #[expect(clippy::too_many_arguments)] // the oracle's own input set
    fn check_invalid_access_face(
        &mut self,
        anchor: SourceRange,
        display: &str,
        target_file: FileId,
        target_container: StringId,
        element_annos: &'a [Annotation],
        container_annos: &'a [Annotation],
        super_member_call: bool,
    ) {
        let Some(decl_lib) = self.graph.library_of_file(target_file) else {
            return;
        };
        let legs = self.access_legs(element_annos, container_annos, decl_lib);
        self.check_invalid_access(
            anchor,
            display,
            target_file,
            target_container,
            legs,
            super_member_call,
        );
    }

    /// The `_checkForOtherInvalidAccess` chain over one resolved
    /// reference: the four legs' verdicts plus whether the access is a
    /// `super.` member read.
    fn check_invalid_access(
        &mut self,
        anchor: SourceRange,
        display: &str,
        target_file: FileId,
        target_container: StringId,
        legs: AccessLegs,
        super_member_call: bool,
    ) {
        let graph = self.graph;
        // `_inCurrentLibrary` (:1673-1675): a use inside the declaring
        // library — any of its units — is exempt before the annotation
        // consult.
        let Some(decl_lib) = graph.library_of_file(target_file) else {
            return;
        };
        if decl_lib == self.lib {
            return;
        }
        if !legs.any() {
            return;
        }
        // `@protected` (:1826-1832): an access inside a container whose
        // type has the declaring class among its supertypes is the
        // sanctioned use, and the whole chain returns.
        if legs.protected && self.access_within_subclass(anchor.offset, target_file, target_container)
        {
            return;
        }
        // `@visibleForTemplate` (:1834-1839): a template source is exempt.
        if legs.template && graph.uri(self.lib).contains(".template") {
            return;
        }
        // `@visibleForTesting` (:1841-1846): a test-directory source is
        // exempt.
        if legs.testing && self.access_in_test_directory() {
            return;
        }
        // `@visibleForOverriding` (:1850-1863): `super.x` inside the very
        // member that overrides `x` is the sanctioned use.
        if legs.overriding && super_member_call && self.access_overrides_enclosing(anchor.offset, display)
        {
            return;
        }
        let uri = graph.uri(decl_lib).to_owned();
        let declaring = if target_container == StringId::NONE {
            graph.name(decl_lib).to_owned()
        } else {
            self.strings.resolve(target_container).to_owned()
        };
        if legs.protected {
            self.report_invalid_access(
                INVALID_USE_OF_PROTECTED_MEMBER,
                anchor,
                format!(
                    "The member '{display}' can only be used within instance members of subclasses of '{declaring}'."
                ),
            );
        }
        if legs.template {
            let code = SemanticCode::InvalidUseOfVisibleForTemplateMember;
            if !self.access_row_reported(code.code_info(), anchor.offset) {
                self.emit(code, anchor, display, &uri);
            }
        }
        if legs.testing {
            self.report_invalid_access(
                INVALID_USE_OF_VISIBLE_FOR_TESTING_MEMBER,
                anchor,
                format!("The member '{display}' can only be used within '{uri}' or a test."),
            );
        }
        if legs.overriding {
            self.report_invalid_access(
                INVALID_USE_OF_VISIBLE_FOR_OVERRIDING_MEMBER,
                anchor,
                format!("The member '{display}' can only be used for overriding."),
            );
        }
    }

    /// The four annotations the element carries, resolved through the
    /// DECLARING library's import faces (they are written in that
    /// library's source). Package:meta getters read the element's own
    /// metadata only; `@visibleForTemplate` is the angular-meta face with
    /// the enclosing cascade and its `visibleOutsideTemplate` mirror.
    fn access_legs(
        &self,
        element: &[Annotation],
        container: &[Annotation],
        decl_lib: LibraryId,
    ) -> AccessLegs {
        let own = |getter: &str| {
            element.iter().any(|annotation| {
                crate::annotation::is_meta_getter(
                    annotation,
                    getter,
                    self.strings,
                    self.graph,
                    decl_lib,
                )
            })
        };
        let cascaded = |getter: &str| {
            element.iter().chain(container.iter()).any(|annotation| {
                is_angular_meta_getter(annotation, getter, self.strings, self.graph, decl_lib)
            })
        };
        AccessLegs {
            protected: own("protected"),
            template: cascaded("visibleForTemplate") && !cascaded("visibleOutsideTemplate"),
            testing: own("visibleForTesting"),
            overriding: own("visibleForOverriding"),
        }
    }

    /// `_hasTypeOrSuperType(_enclosingClass, definingClass)` (:1826-1829,
    /// :1910-1916): the innermost container enclosing the access has the
    /// declaring container among its supertypes.
    fn access_within_subclass(
        &self,
        offset: u32,
        target_file: FileId,
        target_container: StringId,
    ) -> bool {
        if target_container == StringId::NONE {
            return false;
        }
        let Some(enclosing) = self.enclosing_container_at(offset) else {
            return false;
        };
        let start = (self.file, enclosing);
        if start == (target_file, target_container) {
            return true;
        }
        let alias_hop = |name: StringId, file: FileId, lib: LibraryId| {
            self.alias_clause_hop(name, file, lib)
        };
        let hierarchy = FoldHierarchy {
            graph: self.graph,
            namespaces: self.namespaces,
            index: self.index,
            file: self.file,
            lib: self.lib,
            class_modifiers: self.class_modifiers,
            memo: std::cell::RefCell::default(),
            in_progress: std::cell::RefCell::default(),
            alias_head: Some(&alias_hop),
        };
        hierarchy
            .closure(start)
            .contains(&(target_file, target_container))
    }

    /// `_inTestDirectory` (:1617-1621): the analyzed unit sits in a
    /// `test/` directory — the canonical URI's package-relative path.
    fn access_in_test_directory(&self) -> bool {
        let uri = self.graph.uri(self.lib);
        let path = uri
            .strip_prefix("package:")
            .and_then(|rest| rest.split_once('/'))
            .map_or(uri, |(_, path)| path);
        path.starts_with("test/") || path.contains("/test/")
    }

    /// The `@visibleForOverriding` exemption's second half (:1856-1860):
    /// the innermost member declaration enclosing the access spells the
    /// accessed name (the override of that member is the legal use).
    fn access_overrides_enclosing(&self, offset: u32, display: &str) -> bool {
        self.decl_sites.iter().any(|site| {
            let DeclKind::Member { .. } = site.kind else {
                return false;
            };
            let spelling = self.strings.resolve(site.name);
            spelling.strip_suffix('=').unwrap_or(spelling) == display && {
                let (start, end) = self.declaration_extent(site.range.offset);
                start <= offset && offset < end
            }
        })
    }

    /// The innermost container declaration enclosing `offset` (the
    /// oracle's `_enclosingClass`, set by the class-family visits).
    fn enclosing_container_at(&self, offset: u32) -> Option<StringId> {
        let mut best: Option<(u32, StringId)> = None;
        for site in &self.decl_sites {
            if !matches!(site.kind, DeclKind::Container) || site.range.offset > offset {
                continue;
            }
            let (start, end) = self.declaration_extent(site.range.offset);
            if start <= offset && offset < end && best.is_none_or(|(seen, _)| start >= seen) {
                best = Some((start, site.name));
            }
        }
        best.map(|(_, name)| name)
    }

    /// Whether this access code already stands at the offset — the member
    /// funnel and the post-pass `member_refs` walk overlap on plain reads
    /// (the funnel runs first), so every emission is idempotent at
    /// `(offset, code)`. The scan only runs for an annotated target.
    fn access_row_reported(&self, code: CodeInfo, offset: u32) -> bool {
        self.report
            .usage
            .iter()
            .any(|row| row.range.offset == offset && row.code.name == code.name)
    }

    /// One access-family row, emitted into the usage tail (the
    /// source-ordered post-pass bucket the sibling faces stage into).
    fn report_invalid_access(&mut self, code: CodeInfo, range: SourceRange, message: String) {
        if self.access_row_reported(code, range.offset) {
            return;
        }
        self.report.usage.push(Diagnostic::new(
            code.dtype.severity(),
            code,
            message,
            self.file,
            range,
            None,
        ));
    }

    /// `super` in front of the member selector (`super.x` — the oracle's
    /// `PropertyAccess`/`MethodInvocation` with a `SuperExpression`
    /// target, :1851-1853).
    fn access_is_super_call(&self, offset: u32) -> bool {
        let tokens = self.tokens.as_slice();
        let Some(at) = self.access_token_at(offset) else {
            return false;
        };
        at >= 2
            && tokens[at - 1].kind == TokenKind::Period
            && matches!(
                tokens[at - 2].kind,
                TokenKind::Keyword(Keyword::Super)
            )
    }

    /// The access family over the post-pass publications the member
    /// funnel never reaches: bare references (top-level declarations and
    /// containers), type references (a container, or the constructor a
    /// creation head names), and setter writes.
    fn report_invalid_access_references(&mut self) {
        let strings = self.strings;
        let index = self.index;
        let graph = self.graph;
        let namespaces = self.namespaces;
        let refs = std::mem::take(&mut self.refs);
        for reference in &refs {
            let RefTarget::Decl { file } = reference.target else {
                continue;
            };
            let Some(elements) = index.get(file) else {
                continue;
            };
            // A top-level declaration carries its own metadata; a container
            // reference is the container element the oracle's
            // `verifyNamedType` resolves (a class-level annotation never
            // cascades, so no container face joins it).
            let (annos, container) = match elements.decl(reference.name) {
                Some(decl) => (decl.annotations.as_ref(), StringId::NONE),
                None => match elements.container(reference.name) {
                    Some(decl) => (decl.annotations.as_ref(), reference.name),
                    None => continue,
                },
            };
            self.check_invalid_access_face(
                reference.range,
                strings.resolve(reference.name),
                file,
                container,
                annos,
                &[],
                false,
            );
        }
        self.refs = refs;
        let type_uses = std::mem::take(&mut self.type_uses);
        for (range, name) in &type_uses {
            let dartq_resolution::Lookup::Found(dartq_resolution::NamespaceEntry::Decl(file)) =
                namespaces.lookup(graph, self.lib, *name)
            else {
                continue;
            };
            let Some(elements) = index.get(file) else {
                continue;
            };
            if elements.container(*name).is_none() {
                continue;
            }
            let Some((anchor, display, annos)) = self.type_reference_face(*range, *name, elements)
            else {
                continue;
            };
            self.check_invalid_access_face(anchor, &display, file, *name, annos, &[], false);
        }
        self.type_uses = type_uses;
        // The resolved member edges: the write faces (the read funnel never
        // sees them — `writeOrReadElement` resolves the setter side), the
        // `super.x` reads (a dedicated resolution path upstream of the
        // funnel), and the constructor edges (`_getIdentifierNameAndError
        // Entity`'s ConstructorName join — constructors are no member rows,
        // so the funnel resolves the class and misses the ctor's own star).
        // Overlapping plain reads are absorbed by the per-row idempotence.
        let member_refs = std::mem::take(&mut self.report.member_refs);
        for edge in &member_refs {
            let Some(elements) = index.get(edge.target_file) else {
                continue;
            };
            let (display, annos) = match member_entry_annotations(
                elements,
                edge.target_container,
                edge.target_name,
            ) {
                Some(annos) => {
                    let spelling = strings.resolve(edge.target_name);
                    (
                        spelling.strip_suffix('=').unwrap_or(spelling).to_owned(),
                        annos,
                    )
                }
                // A constructor edge names a selector no member row carries;
                // the `A.named` display is the oracle's
                // `element.dart:838-847` join.
                None if !matches!(
                    edge.kind,
                    MemberRefKind::Written | MemberRefKind::ReadWritten
                ) && edge.target_name != edge.target_container =>
                {
                    let Some(annos) =
                        elements.ctor_annotations(edge.target_container, edge.target_name)
                    else {
                        continue;
                    };
                    (
                        format!(
                            "{}.{}",
                            strings.resolve(edge.target_container),
                            strings.resolve(edge.target_name)
                        ),
                        annos,
                    )
                }
                None => continue,
            };
            let super_member_call = self.access_is_super_call(edge.range.offset);
            self.check_invalid_access_face(
                edge.range,
                &display,
                edge.target_file,
                edge.target_container,
                annos,
                &[],
                super_member_call,
            );
        }
        self.report.member_refs = member_refs;
    }

    /// The access face of one type position: a creation head
    /// (`A()`, `A.named()`, `@A()`) resolves the CONSTRUCTOR element —
    /// `_getIdentifierNameAndErrorEntity`'s ConstructorName join
    /// (:2011-2032) — while every other named type resolves its container.
    fn type_reference_face(
        &self,
        range: SourceRange,
        name: StringId,
        elements: &'a crate::model::FileElements,
    ) -> Option<(SourceRange, String, &'a [Annotation])> {
        let tokens = self.tokens.as_slice();
        let leaf = self.access_token_at(range.offset)?;
        let next = tokens.get(leaf + 1).map(|t| t.kind);
        let selector = match (
            next,
            tokens.get(leaf + 2).map(|t| t.kind),
            tokens.get(leaf + 3).map(|t| t.kind),
        ) {
            (Some(TokenKind::Period), Some(TokenKind::Identifier), Some(TokenKind::OpenParen)) => {
                Some(leaf + 2)
            }
            _ => None,
        };
        let container_spelling = self.strings.resolve(name).to_owned();
        if let Some(at) = selector {
            let spelling = self.strings.intern(self.tokens.lexeme(self.source, at));
            let annos = elements.ctor_annotations(name, spelling)?;
            let end = tokens[at].utf16_offset + self.access_token_len(at);
            return Some((
                SourceRange {
                    offset: range.offset,
                    length: end - range.offset,
                },
                format!("{container_spelling}.{}", self.strings.resolve(spelling)),
                annos,
            ));
        }
        // The unnamed creation head: `displayName` is the class spelling
        // alone (element.dart:838-847); a synthesized default constructor
        // carries the class's own metadata.
        let ctor_annos = (next == Some(TokenKind::OpenParen))
            .then(|| elements.ctor_annotations(name, name))
            .flatten()
            .filter(|annos| !annos.is_empty());
        let annos = match ctor_annos {
            Some(annos) => annos,
            None => elements.container(name)?.annotations.as_ref(),
        };
        Some((range, container_spelling, annos))
    }

    /// `_checkForTypeAnnotationDeferredClass` (`error_verifier.dart`
    /// :7132-7139): a `NamedType` whose written prefix resolves to a
    /// deferred import reports at the type node. The oracle's ten visitor
    /// sites (:433 `as`, :512 `catch`, :1127 field-formal parameter,
    /// :1245 function return, :1435 `is`, :1525 method return, :1855
    /// formal parameter, :2146 type argument, :2157 type-parameter bound,
    /// :2199 variable list) cover every named type in a type position
    /// except a function type's return and the supertype clauses, which
    /// this pass excludes.
    pub(crate) fn check_type_annotation_deferred_class(&mut self) {
        let graph = self.graph;
        let strings = self.strings;
        // `NamedType.isDeferred` (:23307-23317): the written prefix's
        // element carries a `deferred` fragment — the import edge flag.
        let mut deferred = FxHashSet::default();
        for edge in graph.import_edges(self.lib) {
            if edge.deferred && let Some(prefix) = edge.prefix {
                deferred.insert(prefix);
            }
        }
        if deferred.is_empty() {
            return;
        }
        let clauses = self.supertype_clause_spans();
        let type_uses = std::mem::take(&mut self.type_uses);
        for (range, name) in &type_uses {
            let Some(leaf) = self.access_token_at(range.offset) else {
                continue;
            };
            // `push_type_use` records the LEAF of a written `prefix.leaf`
            // (the prefixed type parse fires the prefix event separately),
            // so the two tokens before it spell the qualifier.
            if leaf < 2 {
                continue;
            }
            let tokens = self.tokens.as_slice();
            if tokens[leaf - 1].kind != TokenKind::Period
                || tokens[leaf - 2].kind != TokenKind::Identifier
            {
                continue;
            }
            let prefix = strings.intern(self.tokens.lexeme(self.source, leaf - 2));
            if !deferred.contains(&prefix) {
                continue;
            }
            let start = tokens[leaf - 2].utf16_offset;
            if clauses
                .iter()
                .any(|&(from, to)| from <= start && start < to)
            {
                continue;
            }
            let (end, after) = self.type_annotation_end(leaf);
            // A function type's return type is checked by no visitor site
            // (`typedef F = a.A Function()`, `void f(a.A g())` keeps the
            // return type out — the parameter's own `type` IS the return
            // annotation, which is why :1855 covers it).
            if tokens
                .get(after)
                .is_some_and(|token| token.kind == TokenKind::Keyword(Keyword::Function))
            {
                continue;
            }
            let qualified = format!(
                "{}.{}",
                self.tokens.lexeme(self.source, leaf - 2),
                strings.resolve(*name)
            );
            self.report.usage.push(Diagnostic::new(
                TYPE_ANNOTATION_DEFERRED_CLASS.dtype.severity(),
                TYPE_ANNOTATION_DEFERRED_CLASS,
                format!(
                    "The deferred type '{qualified}' can't be used in a declaration, cast, or type test."
                ),
                self.file,
                SourceRange {
                    offset: start,
                    length: end - start,
                },
                Some(DEFERRED_CLASS_CORRECTION.to_owned()),
            ));
        }
        self.type_uses = type_uses;
    }

    /// The written extent of the named type whose leaf token sits at
    /// `leaf`: the leaf's own end, extended through its type-argument
    /// group and the `?` suffix (the `NamedType` node's children), with
    /// the index of the token that follows.
    fn type_annotation_end(&self, leaf: usize) -> (u32, usize) {
        let tokens = self.tokens.as_slice();
        let mut end = tokens[leaf].utf16_offset + self.access_token_len(leaf);
        let mut next = leaf + 1;
        if tokens.get(next).is_some_and(|t| t.kind == TokenKind::Lt)
            && let Some(close) = self.angle_group_end(next)
        {
            end = tokens[close].utf16_offset + self.access_token_len(close);
            next = close + 1;
        }
        if tokens.get(next).is_some_and(|t| t.kind == TokenKind::Question) {
            end = tokens[next].utf16_offset + self.access_token_len(next);
            next += 1;
        }
        (end, next)
    }

    /// The declaration-header windows that hold a supertype clause list
    /// (`extends`/`with`/`implements`/`on`), from the clause keyword to the
    /// body opener — the named types no visitor site checks. Angle groups
    /// jump whole, so a type parameter's `extends` bound
    /// (`class C<E extends a.A>`) stays outside every window.
    fn supertype_clause_spans(&self) -> Vec<(u32, u32)> {
        let tokens = self.tokens.as_slice();
        let mut spans = Vec::new();
        for site in &self.decl_sites {
            if !matches!(site.kind, DeclKind::Container) {
                continue;
            }
            let mut clause: Option<u32> = None;
            let mut cursor =
                tokens.partition_point(|t| t.utf16_offset < site.range.offset + site.range.length);
            while let Some(token) = tokens.get(cursor) {
                match token.kind {
                    TokenKind::OpenCurly | TokenKind::Semicolon => {
                        if let Some(from) = clause {
                            spans.push((from, token.utf16_offset));
                        }
                        break;
                    }
                    TokenKind::Lt => match self.angle_group_end(cursor) {
                        Some(close) => cursor = close,
                        None => break,
                    },
                    TokenKind::OpenParen | TokenKind::OpenSquare => {
                        match self.tokens.matching_end(cursor) {
                            Some(close) => cursor = close,
                            None => break,
                        }
                    }
                    TokenKind::Keyword(
                        Keyword::Extends | Keyword::With | Keyword::Implements | Keyword::On,
                    ) => {
                        clause.get_or_insert(token.utf16_offset);
                    }
                    _ => {}
                }
                cursor += 1;
            }
        }
        spans
    }

    /// The token index starting at `offset` (UTF-16), `None` when no token
    /// begins there.
    fn access_token_at(&self, offset: u32) -> Option<usize> {
        let tokens = self.tokens.as_slice();
        let index = tokens.partition_point(|t| t.utf16_offset < offset);
        (tokens.get(index).is_some_and(|t| t.utf16_offset == offset)).then_some(index)
    }

    /// The UTF-16 length of the token at `index`.
    fn access_token_len(&self, index: usize) -> u32 {
        dartq_source::utf16_len_of(self.tokens.lexeme(self.source, index))
    }
}

/// The member annotations an edge's target carries: the declaring
/// container's member row, else a same-named extension face (the
/// `check_target_annotations` lookup, narrowed to the write face).
fn member_entry_annotations<'e>(
    elements: &'e crate::model::FileElements,
    container: StringId,
    name: StringId,
) -> Option<&'e [Annotation]> {
    elements
        .container(container)
        .and_then(|decl| decl.members.get(&name))
        .map(|entry| entry.annotations.as_ref())
        .or_else(|| {
            elements.extension_faces().iter().find_map(|face| {
                (face.name == Some(container) || (face.name.is_none() && container == StringId::NONE))
                    .then(|| face.members.get(&name))
                    .flatten()
                    .map(|entry| entry.annotations.as_ref())
            })
        })
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
        // The #2993 faces ride the same post-pass-2 seam: the whole-unit
        // annotation-target (which drives the directive pass on its own
        // tail), parameter-final, and set-literal walks over the final
        // resolution products.
        self.check_annotation_targets();
        self.check_unnecessary_final_parameters();
        self.check_missing_required_params();
        self.check_unnecessary_set_literals();
        // The #3357 access-annotation family (the `@protected` /
        // `@visibleForTesting` / `@visibleForOverriding` legs over the
        // references the member funnel never reaches) and the
        // `type_annotation_deferred_class` arm, over the same final
        // resolution products.
        self.report_invalid_access_references();
        self.check_type_annotation_deferred_class();
        // The #3353 `deprecated_*` family (the faces whose only trace used to
        // be the severity registry): the declaration clauses, the creations,
        // and the omitted-optional-argument calls over the same final
        // resolution products.
        self.report_deprecated_functionality();
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
                    //
                    // The "no container" sentinel (`StringId::NONE`, an
                    // unnamed extension's member edge whose face scan
                    // missed the spelling) names no constructor owner —
                    // `ctor_annotations` is keyed by a container name — so
                    // the arm declines instead of resolving the sentinel.
                    if target_container == StringId::NONE {
                        return None;
                    }
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

// —— the unnecessary_set_literal face (#2993) ———————————————————————————
//
// The oracle's `unnecessary_set_literal` warning (upstream Dart SDK source,
// pkg/analyzer/lib/src/error/best_practices_verifier.dart:1353-1391,
// `_checkForUnnecessarySetLiteral`, driven from `visitFunctionExpression`
// :502-509): an expression (`=>`) function body whose expression is a SET
// literal, under a void-returning function type, reports
// `unnecessary_set_literal` at the literal. Two faces reach the void
// verdict:
//
//   * the corresponding-parameter face (:1363-1368): the FunctionExpression
//     is an invocation argument paired with a formal whose declared type
//     is a `FunctionType` — the verdict reads its return type;
//   * the declared-return face (:1369-1372): the FunctionExpression's
//     parent is a `FunctionDeclaration` (a top-level function/getter/
//     setter or a local function — methods are `MethodDeclaration`s and
//     never ride this face) with a declared return type.
//
// Void-shape (:1375-1386): `void`, or `Future<void>`/`FutureOr<void>`
// (exactly one type argument, itself `void`). A closure in any other
// position (an initializer, a return, a collection element) has no
// corresponding parameter and no `FunctionDeclaration` parent — no report.
//
// Name-level mirror: the declared-return face walks this unit's top-level
// executable declaration sites (local functions are under-reports — the
// library-level site table is the only declaration inventory the model
// keeps), and the corresponding-parameter face walks the same three call
// faces the `missing_required_param` pass rides (bare `refs`,
// `member_refs` edges, creation `type_uses`), pairing each argument
// element to its formal and reading the formal's `Ty::Function` shape.

impl Engine<'_> {
    /// The whole-unit `unnecessary_set_literal` pass: the declared-return
    /// face over this unit's top-level executables and the
    /// corresponding-parameter face over every resolved call's closure
    /// arguments.
    pub(crate) fn check_unnecessary_set_literals(&mut self) {
        let findings = self.scan_set_literal_faces();
        for span in findings {
            // The correction template carries no placeholder — the
            // verbatim text through the correction-carrying emit.
            self.emit_corrected(SemanticCode::UnnecessarySetLiteral, span, "");
        }
    }

    /// Both faces' report spans, collected before any emission (one
    /// immutable walk; `emit` mutates).
    fn scan_set_literal_faces(&self) -> Vec<SourceRange> {
        let mut findings = Vec::new();
        let index = self.index;
        let Some(elements) = index.get(self.file) else {
            return findings;
        };
        // The declared-return face: a top-level function/getter with a
        // declared void-shape return and an arrow set-literal body
        // (`void f() => {1}`, `void get g => {1}`).
        let pool = elements.type_args();
        for site in &self.decl_sites {
            if !matches!(site.kind, DeclKind::TopLevelFn) {
                continue;
            }
            let Some(decl) = elements.decl(site.name) else {
                continue;
            };
            if !self.ty_is_void_return(&decl.return_ty, pool) {
                continue;
            }
            if let Some(span) = self.arrow_set_literal_body(site.range) {
                findings.push(span);
            }
        }
        // The corresponding-parameter face: every resolved call's
        // arguments, each closure element paired with its formal.
        for reference in &self.refs {
            let RefTarget::Decl { file } = reference.target else {
                continue;
            };
            if let Some((formals, open)) = self
                .member_or_decl_formals(file, None, reference.name)
                .and_then(|formals| Some((formals, self.call_open_paren(reference.range)?)))
                && let Some(decl_elements) = index.get(file)
            {
                findings.extend(self.scan_closure_argument_elements(
                    open,
                    formals,
                    decl_elements,
                ));
            }
        }
        for edge in &self.report.member_refs {
            if let Some((formals, open)) = self
                .member_or_decl_formals(
                    edge.target_file,
                    Some(edge.target_container),
                    edge.target_name,
                )
                .and_then(|formals| Some((formals, self.call_open_paren(edge.range)?)))
                && let Some(decl_elements) = index.get(edge.target_file)
            {
                findings.extend(self.scan_closure_argument_elements(
                    open,
                    formals,
                    decl_elements,
                ));
            }
        }
        for (range, name) in &self.type_uses {
            if let Some((formals, open, file, _)) = self.creation_face(*range, *name)
                && let Some(decl_elements) = index.get(file)
            {
                findings.extend(self.scan_closure_argument_elements(
                    open,
                    formals,
                    decl_elements,
                ));
            }
        }
        findings
    }

    /// The void-shape verdict of `_checkForUnnecessarySetLiteral`
    /// (:1375-1386): `void`, or `Future<void>`/`FutureOr<void>` — exactly
    /// one type argument, itself `void`. A same-named local declaration
    /// shadows the dart:async heads (the oracle's `isDartAsyncFuture`
    /// reads the resolved element, never the spelling alone).
    fn ty_is_void_return(&self, ty: &Ty, pool: &ArgPool) -> bool {
        match ty {
            Ty::Void => true,
            Ty::Named { name, args, .. } => {
                let head = self.strings.resolve(*name);
                if head != "Future" && head != "FutureOr" {
                    return false;
                }
                matches!(self.graph.local(self.lib, *name), LocalLookup::Absent)
                    && pool.get(*args).len() == 1
                    && matches!(pool.get(*args)[0], Ty::Void)
            }
            _ => false,
        }
    }

    /// A declaration-site arrow body that is a set literal: from the name
    /// token, the optional type arguments, the optional formal group, the
    /// optional async/sync marker, then `=>` and a `{…}` body — the
    /// report span when the braces spell a set. `None` for every other
    /// shape (a block body, a non-literal arrow expression, no arrow).
    fn arrow_set_literal_body(&self, name_range: SourceRange) -> Option<SourceRange> {
        let tokens = self.tokens.as_slice();
        let mut cursor =
            tokens.partition_point(|t| t.utf16_offset < name_range.offset + name_range.length);
        if tokens.get(cursor).is_some_and(|t| t.kind == TokenKind::Lt) {
            cursor = self.angle_group_end(cursor)? + 1;
        }
        if tokens.get(cursor).is_some_and(|t| t.kind == TokenKind::OpenParen) {
            cursor = self.tokens.matching_end(cursor)? + 1;
        }
        cursor = self.skip_arrow_marker(cursor);
        if !tokens.get(cursor).is_some_and(|t| t.kind == TokenKind::Function) {
            return None;
        }
        cursor += 1;
        if !tokens.get(cursor).is_some_and(|t| t.kind == TokenKind::OpenCurly) {
            return None;
        }
        let close = self.tokens.matching_end(cursor)?;
        self.braces_span_if_set(cursor, close)
    }

    /// The optional `async`/`async*`/`sync*` marker tokens before an
    /// arrow, from `cursor`, landing just past the marker.
    fn skip_arrow_marker(&self, mut cursor: usize) -> usize {
        let tokens = self.tokens.as_slice();
        while matches!(
            tokens.get(cursor).map(|t| t.kind),
            Some(TokenKind::Keyword(Keyword::Async)) | Some(TokenKind::Keyword(Keyword::Sync))
        ) {
            cursor += 1;
            if tokens.get(cursor).is_some_and(|t| t.kind == TokenKind::Star) {
                cursor += 1;
            }
        }
        cursor
    }

    /// One call's argument elements: each closure element
    /// (`(…) [async] => {…}`) paired with its formal (a named label join,
    /// or the positional slot), reporting when the paired formal's
    /// function type is void-returning and the arrow body is a set
    /// literal — `correspondingParameter?.type` (:1363-1368) at name
    /// level.
    fn scan_closure_argument_elements(
        &self,
        open: usize,
        formals: &Formals,
        elements: &FileElements,
    ) -> Vec<SourceRange> {
        let pool = elements.type_args();
        let tokens = self.tokens.as_slice();
        let Some(close) = self.tokens.matching_end(open) else {
            return Vec::new();
        };
        let mut findings = Vec::new();
        let mut cursor = open + 1;
        let mut positional = 0usize;
        while cursor < close {
            // One top-level element: [start, end), split at depth-0
            // commas (brackets only — angle groups never open argument
            // depth).
            let start = cursor;
            let mut depth = 0i32;
            while cursor < close {
                match tokens[cursor].kind {
                    TokenKind::OpenParen | TokenKind::OpenCurly | TokenKind::OpenSquare => {
                        depth += 1;
                    }
                    TokenKind::CloseParen | TokenKind::CloseCurly | TokenKind::CloseSquare => {
                        depth -= 1;
                    }
                    TokenKind::Comma if depth == 0 => break,
                    _ => {}
                }
                cursor += 1;
            }
            // The paired formal: a `name:` head joins the named table,
            // any other element takes the next positional slot.
            let (content_start, formal): (usize, Option<&Formal>) =
                if tokens[start].kind == TokenKind::Identifier
                    && tokens.get(start + 1).is_some_and(|t| t.kind == TokenKind::Colon)
                {
                    let label = self.strings.intern(self.tokens.lexeme(self.source, start));
                    (start + 2, formals.named_formal(label))
                } else {
                    let formal = formals.positional.get(positional);
                    positional += 1;
                    (start, formal)
                };
            if let Some(formal) = formal
                && let Some(span) =
                    self.closure_arrow_set_literal(content_start, formal, pool)
            {
                findings.push(span);
            }
            cursor += 1;
        }
        findings
    }

    /// A closure argument's arrow set-literal body under a void-returning
    /// formal type: the element at `start` spells a closure head
    /// (`(…) [async] =>`), its body braces a set literal, and the formal's
    /// function shape returns the void shape.
    fn closure_arrow_set_literal(
        &self,
        start: usize,
        formal: &Formal,
        pool: &ArgPool,
    ) -> Option<SourceRange> {
        let tokens = self.tokens.as_slice();
        let mut cursor = start;
        if tokens.get(cursor)?.kind != TokenKind::OpenParen {
            return None;
        }
        cursor = self.tokens.matching_end(cursor)? + 1;
        cursor = self.skip_arrow_marker(cursor);
        if tokens.get(cursor)?.kind != TokenKind::Function {
            return None;
        }
        cursor += 1;
        if tokens.get(cursor)?.kind != TokenKind::OpenCurly {
            return None;
        }
        let close = self.tokens.matching_end(cursor)?;
        let Ty::Function { shape, .. } = formal.ty else {
            return None;
        };
        if !self.ty_is_void_return(&pool.function_shape(shape).return_ty, pool) {
            return None;
        }
        self.braces_span_if_set(cursor, close)
    }

    /// The braces' span when they spell a SET literal (`SetOrMapLiteral
    /// .isSet` at token level): at least one element, and no element
    /// carries a top-level map-entry colon. A colon behind a conditional
    /// (`c ? a : b`) belongs to the conditional — its `?` is a top-level
    /// marker that is neither the element's null-aware prefix (`?k: v`)
    /// nor a spread's (`...?a ? b : c`) — and a null-aware element start
    /// stays a map marker (`{?k: v}`).
    fn braces_span_if_set(&self, open: usize, close: usize) -> Option<SourceRange> {
        let tokens = self.tokens.as_slice();
        if open + 1 >= close {
            // `{}` resolves as a map (no elements, no context).
            return None;
        }
        let mut depth = 0i32;
        let mut element_start = true;
        let mut conditional_open = false;
        let mut prev = TokenKind::Eof;
        for index in (open + 1)..close {
            let token = tokens[index];
            match token.kind {
                TokenKind::OpenParen | TokenKind::OpenCurly | TokenKind::OpenSquare => {
                    depth += 1;
                }
                TokenKind::CloseParen | TokenKind::CloseCurly | TokenKind::CloseSquare => {
                    depth -= 1;
                }
                TokenKind::Comma if depth == 0 => {
                    element_start = true;
                    conditional_open = false;
                    prev = TokenKind::Eof;
                    continue;
                }
                TokenKind::Question if depth == 0 => {
                    if !element_start
                        && prev != TokenKind::PeriodPeriodPeriod
                        && prev != TokenKind::PeriodPeriodPeriodQuestion
                    {
                        conditional_open = true;
                    }
                }
                TokenKind::Colon if depth == 0 => {
                    if !conditional_open {
                        // A map-entry colon.
                        return None;
                    }
                }
                _ => {}
            }
            element_start = false;
            prev = token.kind;
        }
        Some(SourceRange {
            offset: tokens[open].utf16_offset,
            length: tokens[close].utf16_offset + 1 - tokens[open].utf16_offset,
        })
    }
}
