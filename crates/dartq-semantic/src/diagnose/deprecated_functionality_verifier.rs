//! The `deprecated_*` functionality faces (#3353) — the verifier family whose
//! only trace in this crate used to be the severity registry. Upstream Dart
//! SDK source: pkg/analyzer/lib/src/error/
//! `deprecated_functionality_verifier.dart` (the declaration and creation
//! faces) plus `best_practices_verifier.dart`'s `_checkForBadFunctionUse`
//! (:3157-3203, the `Function` arms).
//!
//! # The official decision, mirrored
//!
//! Every face is one question — `element.isDeprecatedWithKind(kind)`
//! (element.dart:2300-2302: the element's own metadata carries a dart:core
//! `@Deprecated` tag whose kind is `kind`; the kind is read off the computed
//! constant by `ElementAnnotationImpl.deprecationKind`, :1785-1793) — asked
//! at a different syntax:
//!
//! | face | syntax | code |
//! | --- | --- | --- |
//! | `_checkForDeprecatedExtend` | `class C extends S` | `deprecated_extend` / `deprecated_subclass` |
//! | `_checkForDeprecatedImplement` | `implements I`, a mixin's `on C` | `deprecated_implement` / `deprecated_subclass` |
//! | `_checkForDeprecatedMixin` | `with M` | `deprecated_mixin` |
//! | `_checkForDeprecatedSubclass` | a class's `with` list (its twin call, :24-29) | `deprecated_subclass` |
//! | `_checkForDeprecatedInstantiate` | the `ConstructorName` of a creation, a redirecting factory, or a constructor tear-off | `deprecated_instantiate` |
//! | `_checkForDeprecatedOptional` | a call omitting an `@Deprecated.optional` formal | `deprecated_optional` |
//! | `_checkForBadFunctionUse` | `extends`/`implements`/`with` of dart:core `Function`, pre-3.0 language only | `deprecated_subtype_of_function` |
//!
//! The declaration faces carry the oracle's two gates before the kind test:
//! the target must resolve to an interface (`node.type?.element is
//! InterfaceElement` — a class, enum, or mixin row here) and a same-library
//! target is exempt (`element.library == _currentLibrary`). Neither the
//! creation face nor the omission face carries a library gate.
//!
//! # Anchors (the oracle's, verbatim)
//!
//! A clause row anchors the written `NamedType` — its head through the last
//! type argument — recovered by a header token scan, because the model keeps
//! the clause NAMES and never their spans. A creation row anchors the
//! `ConstructorName` (`Foo`, `Foo.named`, `Foo.new`). An omission row anchors
//! the callee (the reference's own span).
//!
//! # Known under-reports (documented; never a false-positive direction)
//!
//! `this(…)`/`super(…)` redirections, super-parameters, primary constructors,
//! and factory redirect parameters (the `_checkForDeprecatedOptional*`
//! siblings); dot-shorthand creations (`dotShorthandConstructorInvocation`);
//! constructor tear-offs (`Foo.new` — the resolution products publish no edge
//! for one, so the `ConstructorName` cannot be recovered here); and the
//! doc-comment `deprecated_new_in_comment_reference` face (this crate models
//! no doc comments). Only the callee-anchored omission shapes ship here.

use super::*;
use crate::annotation::DeprecationKind;
use crate::model::{Container, ContainerKind, MicroKind};
use dartq_resolution::{LocalLookup, Lookup, NamespaceEntry};
use dartq_scanner::TokenKind;

// —— the family codes ————————————————————————————————————————————————————
//
// Registry rows (pkg/analyzer/lib/src/diagnostic/diagnostic.g.dart): every
// one is a `STATIC_WARNING` with published docs. The three `Function` arms
// share the printed name `deprecated_subtype_of_function` and differ only in
// their messages (:3726-3735, :3786-3796, :3905-3914).

const DEPRECATED_EXTEND: CodeInfo = CodeInfo::new(
    "deprecated_extend",
    "DEPRECATED_EXTEND",
    DiagnosticType::StaticWarning,
    true,
);
const DEPRECATED_IMPLEMENT: CodeInfo = CodeInfo::new(
    "deprecated_implement",
    "DEPRECATED_IMPLEMENT",
    DiagnosticType::StaticWarning,
    true,
);
const DEPRECATED_INSTANTIATE: CodeInfo = CodeInfo::new(
    "deprecated_instantiate",
    "DEPRECATED_INSTANTIATE",
    DiagnosticType::StaticWarning,
    true,
);
const DEPRECATED_MIXIN: CodeInfo = CodeInfo::new(
    "deprecated_mixin",
    "DEPRECATED_MIXIN",
    DiagnosticType::StaticWarning,
    true,
);
const DEPRECATED_OPTIONAL: CodeInfo = CodeInfo::new(
    "deprecated_optional",
    "DEPRECATED_OPTIONAL",
    DiagnosticType::StaticWarning,
    true,
);
const DEPRECATED_SUBCLASS: CodeInfo = CodeInfo::new(
    "deprecated_subclass",
    "DEPRECATED_SUBCLASS",
    DiagnosticType::StaticWarning,
    true,
);
const DEPRECATED_SUBTYPE_OF_FUNCTION: CodeInfo = CodeInfo::new(
    "deprecated_subtype_of_function",
    "DEPRECATED_SUBTYPE_OF_FUNCTION",
    DiagnosticType::StaticWarning,
    true,
);

/// One collected row: the code, the anchor, the rendered message, and the
/// registry's `correctionMessage` with its `{0}` substituted.
type DepRow = (CodeInfo, SourceRange, String, Option<String>);

/// A supertype clause of a declaration header.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Clause {
    Extends,
    Implements,
    With,
    /// The `classDeclaration` twin call that reads the `with` list for the
    /// `subclass` kind (`_checkForDeprecatedSubclass`, :24-29).
    WithSubclass,
    On,
}

impl Clause {
    fn from_keyword(text: &str) -> Option<Self> {
        Some(match text {
            "extends" => Self::Extends,
            "implements" => Self::Implements,
            "with" => Self::With,
            "on" => Self::On,
            _ => return None,
        })
    }

    /// The clause whose written entries this column carries.
    const fn scan_face(self) -> Self {
        match self {
            Self::WithSubclass => Self::With,
            other => other,
        }
    }

    /// Whether `_checkForBadFunctionUse` reads this clause
    /// (best_practices_verifier.dart:3188-3203: superclass, interfaces,
    /// mixins — never a mixin's `on` constraints).
    const fn takes_function_face(self) -> bool {
        matches!(self, Self::Extends | Self::Implements | Self::With)
    }

    /// The `Function` arm's verbatim message and correction.
    const fn function_message(self) -> (&'static str, &'static str) {
        match self {
            Self::Implements => (
                "Implementing 'Function' has no effect.",
                "Try removing 'Function' from the 'implements' clause.",
            ),
            Self::With | Self::WithSubclass => (
                "Mixing in 'Function' is deprecated.",
                "Try removing 'Function' from the 'with' clause.",
            ),
            Self::Extends | Self::On => (
                "Extending 'Function' is deprecated.",
                "Try removing 'Function' from the 'extends' clause.",
            ),
        }
    }

    /// The row this clause emits for a deprecation `kind`: each arm tests its
    /// own kind, so the `subclass` arms of `extends`/`implements` report that
    /// code while the same clause's own kind reports its own (:150-183).
    fn row_for(
        self,
        kind: Option<DeprecationKind>,
        name: &str,
    ) -> Option<(CodeInfo, String, Option<String>)> {
        let kind = kind?;
        Some(match (self, kind) {
            (Self::Extends, DeprecationKind::Extend) => (
                DEPRECATED_EXTEND,
                format!("Extending '{name}' is deprecated."),
                Some("Try removing the 'extends' clause.".to_owned()),
            ),
            (
                Self::Extends | Self::Implements | Self::On | Self::WithSubclass,
                DeprecationKind::Subclass,
            ) => (
                DEPRECATED_SUBCLASS,
                format!("Subclassing '{name}' is deprecated."),
                Some(format!(
                    "Try removing the 'extends' clause, or removing '{name}' from the 'implements' clause."
                )),
            ),
            (Self::Implements | Self::On, DeprecationKind::Implement) => (
                DEPRECATED_IMPLEMENT,
                format!("Implementing '{name}' is deprecated."),
                Some(format!(
                    "Try removing '{name}' from the 'implements' clause."
                )),
            ),
            (Self::With, DeprecationKind::Mixin) => (
                DEPRECATED_MIXIN,
                format!("Mixing in '{name}' is deprecated."),
                Some(format!("Try removing '{name}' from the 'with' clause.")),
            ),
            _ => return None,
        })
    }
}

impl Engine<'_> {
    /// The whole family's post-pass-2 pass, run from
    /// [`Engine::report_member_use_warnings`] beside the member-use faces it
    /// shares its resolution products with.
    pub(crate) fn report_deprecated_functionality(&mut self) {
        let mut rows = self.deprecated_clause_rows();
        rows.extend(self.deprecated_instantiation_rows());
        rows.extend(self.deprecated_optional_rows());
        // One row per (anchor, code): the clause scan meets a class-type
        // alias's head through the `=` arm only, and a creation rides both
        // the type-use and the member-ref publication.
        let mut reported: FxHashSet<(u32, &'static str)> = FxHashSet::default();
        for (code, range, message, correction) in rows {
            if !reported.insert((range.offset, code.name)) {
                continue;
            }
            self.report.usage.push(Diagnostic::new(
                code.dtype.severity(),
                code,
                message,
                self.file,
                range,
                correction,
            ));
        }
    }

    /// The declaration faces over this unit's containers, in source order.
    fn deprecated_clause_rows(&self) -> Vec<DepRow> {
        let mut rows = Vec::new();
        let Some(elements) = self.index.get(self.file) else {
            return rows;
        };
        // `_checkForBadFunctionUse` bails when the library has the
        // class-modifiers feature (best_practices_verifier.dart:3183-3186:
        // `Function` is `final` there, so `extends Function` is an
        // outside-library error instead).
        let function_faces = !self.class_modifiers.is_enabled(self.lib);
        for site in &self.decl_sites {
            if !matches!(site.kind, DeclKind::Container) {
                continue;
            }
            let Some(container) = elements.container(site.name) else {
                continue;
            };
            // `visitEnumDeclaration` reads implements + with,
            // `visitMixinDeclaration` implements + on, `visitClassDeclaration`
            // (and its class-type-alias twin) the full quartet; an
            // extension-type declaration carries no visit upstream.
            let (extends, implements, with, on) = match container.kind {
                ContainerKind::Class => (true, true, true, false),
                ContainerKind::Enum => (false, true, true, false),
                ContainerKind::Mixin => (false, true, false, true),
                ContainerKind::Extension | ContainerKind::ExtensionType => continue,
            };
            if extends && let Some(head) = container.superclass {
                self.clause_rows(
                    site.name,
                    site.range,
                    Clause::Extends,
                    std::slice::from_ref(&head),
                    function_faces,
                    &mut rows,
                );
            }
            if implements {
                self.clause_rows(
                    site.name,
                    site.range,
                    Clause::Implements,
                    &container.implements_clause,
                    function_faces,
                    &mut rows,
                );
            }
            if with {
                self.clause_rows(
                    site.name,
                    site.range,
                    Clause::With,
                    &container.with_clause,
                    function_faces,
                    &mut rows,
                );
                if container.kind == ContainerKind::Class {
                    self.clause_rows(
                        site.name,
                        site.range,
                        Clause::WithSubclass,
                        &container.with_clause,
                        false,
                        &mut rows,
                    );
                }
            }
            if on {
                self.clause_rows(
                    site.name,
                    site.range,
                    Clause::On,
                    &container.on_clause,
                    false,
                    &mut rows,
                );
            }
        }
        rows
    }

    /// One clause's rows: the n-th written entry pairs the n-th model name
    /// (the clause lists keep written order), so the recovered spans index
    /// them positionally.
    fn clause_rows(
        &self,
        writer: StringId,
        site: SourceRange,
        clause: Clause,
        entries: &[StringId],
        function_faces: bool,
        rows: &mut Vec<DepRow>,
    ) {
        if entries.is_empty() {
            return;
        }
        let spans = self.clause_entry_spans(site, clause.scan_face());
        for (index, written) in entries.iter().enumerate() {
            let Some(span) = spans.get(index).copied() else {
                continue;
            };
            match self.resolve_clause_face(writer, self.file, self.lib, *written) {
                Some((target, target_file)) => {
                    // The same-library exemption: `element.library ==
                    // _currentLibrary` bails before the kind test (:149,
                    // :169, :206).
                    let decl_lib = self.graph.library_of_file(target_file).unwrap_or(self.lib);
                    if decl_lib == self.lib {
                        continue;
                    }
                    // `node.type?.element is InterfaceElement`: an extension
                    // (or extension type) target is no interface.
                    if matches!(
                        target.kind,
                        ContainerKind::Extension | ContainerKind::ExtensionType
                    ) {
                        continue;
                    }
                    let kind = self.container_deprecation_kind(target, decl_lib);
                    let name = self.strings.resolve(*written).to_owned();
                    if let Some((code, message, correction)) = clause.row_for(kind, &name) {
                        rows.push((code, span, message, correction));
                    }
                }
                None => {
                    // A clause face that resolves to no container may still
                    // BE dart:core's `Function` (a built-in type, not a
                    // container row).
                    if function_faces
                        && clause.takes_function_face()
                        && self.denotes_core_function(*written)
                    {
                        let (message, correction) = clause.function_message();
                        rows.push((
                            DEPRECATED_SUBTYPE_OF_FUNCTION,
                            span,
                            message.to_owned(),
                            Some(correction.to_owned()),
                        ));
                    }
                }
            }
        }
    }

    /// The deprecation kind of a resolved container row, read in its
    /// DECLARING library's scope (the file the annotation is written in).
    fn container_deprecation_kind(
        &self,
        container: &Container,
        decl_lib: LibraryId,
    ) -> Option<DeprecationKind> {
        container.annotations.iter().find_map(|annotation| {
            crate::annotation::deprecation_kind(
                annotation,
                self.strings,
                self.graph,
                self.namespaces,
                decl_lib,
            )
        })
    }

    /// The written spans of one clause's entries, in source order. The model
    /// keeps the clause names and no spans, so the entries are re-read off
    /// the header tokens: from the declaring name, skip the type-parameter
    /// group, then collect each `NamedType` under the clause's keyword (a
    /// class type alias's `=` head is the `extends` face). An entry's span
    /// runs from its first identifier through the last type argument — the
    /// oracle's `NamedType.at(…)`.
    fn clause_entry_spans(&self, site: SourceRange, clause: Clause) -> Vec<SourceRange> {
        let tokens = self.tokens.as_slice();
        let mut cursor = tokens.partition_point(|t| t.utf16_offset < site.end());
        if tokens.get(cursor).is_some_and(|t| t.kind == TokenKind::Lt) {
            let Some(end) = self.angle_group_end(cursor) else {
                return Vec::new();
            };
            cursor = end + 1;
        }
        let mut spans = Vec::new();
        let mut current: Option<Clause> = None;
        while let Some(token) = tokens.get(cursor) {
            match token.kind {
                TokenKind::OpenCurly | TokenKind::Semicolon | TokenKind::Eof => break,
                TokenKind::Eq => {
                    current = Some(Clause::Extends);
                    cursor += 1;
                }
                TokenKind::Keyword(_) | TokenKind::Identifier => {
                    let text = self.tokens.lexeme(self.source, cursor);
                    if let Some(keyword) = Clause::from_keyword(text) {
                        current = Some(keyword);
                        cursor += 1;
                        continue;
                    }
                    let (span, next) = self.type_span_at(cursor);
                    if current == Some(clause)
                        && let Some(span) = span
                    {
                        spans.push(span);
                    }
                    cursor = next;
                }
                _ => cursor += 1,
            }
        }
        spans
    }

    /// The span of the `NamedType` opened at token `start` (`a.b.C<int>`),
    /// and the index just past it.
    fn type_span_at(&self, start: usize) -> (Option<SourceRange>, usize) {
        let tokens = self.tokens.as_slice();
        let Some(token) = tokens.get(start) else {
            return (None, start + 1);
        };
        let is_name =
            |kind: TokenKind| matches!(kind, TokenKind::Identifier | TokenKind::Keyword(_));
        if !is_name(token.kind) {
            return (None, start + 1);
        }
        let mut last = start;
        let mut cursor = start + 1;
        while tokens
            .get(cursor)
            .is_some_and(|t| t.kind == TokenKind::Period)
            && tokens.get(cursor + 1).is_some_and(|t| is_name(t.kind))
        {
            last = cursor + 1;
            cursor += 2;
        }
        if tokens.get(cursor).is_some_and(|t| t.kind == TokenKind::Lt) {
            match self.angle_group_end(cursor) {
                Some(end) => {
                    last = end;
                    cursor = end + 1;
                }
                None => return (None, cursor),
            }
        }
        let offset = tokens[start].utf16_offset;
        let end = tokens[last].utf16_offset + self.token_utf16_len(last);
        (
            Some(SourceRange {
                offset,
                length: end - offset,
            }),
            cursor,
        )
    }

    /// The UTF-16 length of the token at `index` (an identifier's lexeme is
    /// the only wide-token case a span walk meets).
    fn token_utf16_len(&self, index: usize) -> u32 {
        dartq_source::utf16_len_of(self.tokens.lexeme(self.source, index))
    }

    /// Whether the written clause face denotes dart:core's `Function` —
    /// `_checkForBadFunctionUse`'s `type.isDartCoreFunction`, alias
    /// transparent (`class A extends F` over `typedef F = Function;` reports).
    fn denotes_core_function(&self, face: StringId) -> bool {
        let mut head = face;
        for _ in 0..8 {
            let text = self.strings.resolve(head);
            if text == "Function" {
                // A local declaration shadows the implicit core import (the
                // `test_local_language219` face declares `class Function`).
                if matches!(self.graph.local(self.lib, head), LocalLookup::Shadowed) {
                    return false;
                }
                return match self.namespaces.lookup(self.graph, self.lib, head) {
                    Lookup::Found(NamespaceEntry::Decl(file)) => self
                        .graph
                        .library_of_file(file)
                        .is_some_and(|lib| self.graph.uri(lib) == "dart:core"),
                    // The rig posture: a graph carrying no `dart:core` node
                    // models no implicit import, so the spelling IS the core
                    // class (the `crate::annotation` tag posture).
                    Lookup::NotFound => !self
                        .graph
                        .libraries()
                        .any(|lib| self.graph.uri(lib) == "dart:core"),
                    Lookup::Found(NamespaceEntry::Prefix) | Lookup::Ambiguous(_) => false,
                };
            }
            let Some(next) = self.alias_rhs_of(head) else {
                return false;
            };
            if next == head {
                return false;
            }
            head = next;
        }
        false
    }

    /// The right-hand container name of a typedef `head` (`typedef F =
    /// Function`), for the alias-transparent clause faces.
    fn alias_rhs_of(&self, head: StringId) -> Option<StringId> {
        let file = match self.namespaces.lookup(self.graph, self.lib, head) {
            Lookup::Found(NamespaceEntry::Decl(file)) => file,
            _ => return None,
        };
        let decl = self.index.get(file)?.decl(head)?;
        if decl.kind != MicroKind::TypeAlias {
            return None;
        }
        match &decl.var_ty {
            Ty::Named { name, .. } => Some(*name),
            _ => None,
        }
    }
}

impl Engine<'_> {
    /// The `deprecated_instantiate` face: every creation-shaped
    /// `ConstructorName` whose interface carries the `instantiate` kind. The
    /// `visitConstructorName` node covers an instance creation's name, a
    /// redirecting factory's target (`= Foo.two`) and a constructor tear-off
    /// (`Foo.new`) — a `this(…)`/`super(…)` invocation is neither, and the
    /// face carries no library gate (:188-199).
    fn deprecated_instantiation_rows(&self) -> Vec<DepRow> {
        let mut rows = Vec::new();
        for (range, name) in &self.type_uses {
            let Some(span) = self.constructor_name_at(*range) else {
                continue;
            };
            // The alias guard: the type-use publication resolves a typedef
            // head through to its right-hand name, while the oracle's
            // `node.constructorName.type.element is! InterfaceElement` bails
            // on the alias itself (`test_annotatedClass_typedef` expects no
            // row). A head whose written last segment differs from the
            // resolved name is exactly that alias face.
            if !self.head_spells_resolved(*range, *name) {
                continue;
            }
            let Some((container, decl_lib)) = self.resolved_container_row(*name) else {
                continue;
            };
            // A typedef target is no interface element (the oracle's
            // `node.constructorName.type.element is! InterfaceElement` bail),
            // which is why the plain and tear-off typedef faces stay silent.
            if self.container_deprecation_kind(container, decl_lib)
                != Some(DeprecationKind::Instantiate)
            {
                continue;
            }
            let text = self.strings.resolve(*name).to_owned();
            rows.push((
                DEPRECATED_INSTANTIATE,
                span,
                format!("Instantiating '{text}' is deprecated."),
                Some("Try instantiating a non-abstract class.".to_owned()),
            ));
        }
        // The constructor tear-off (`var x = Foo.new;`) is NOT reachable
        // here: the resolution products publish no edge for it (neither a
        // type use nor a member ref), so the `ConstructorName` cannot be
        // recovered. That publication is the resolution lane's; until it
        // lands the `test_annotatedClass_tearoff` row stays an under-report.
        rows
    }

    /// Whether the head written at `range` spells the resolved `name` — the
    /// alias guard ([`Self::deprecated_instantiation_rows`]).
    fn head_spells_resolved(&self, range: SourceRange, name: StringId) -> bool {
        let tokens = self.tokens.as_slice();
        let end = tokens.partition_point(|t| t.utf16_offset < range.offset + range.length);
        let Some(last) = end.checked_sub(1) else {
            return false;
        };
        self.tokens.lexeme(self.source, last) == self.strings.resolve(name)
    }

    /// The `deprecated_optional` face's callee-anchored arms: a bare call, a
    /// member call, and a creation (the `methodInvocation` /
    /// `instanceCreationExpression` visits of :101-126).
    fn deprecated_optional_rows(&self) -> Vec<DepRow> {
        let mut rows = Vec::new();
        // A bare call of a top-level executable (`f()`).
        for reference in &self.refs {
            let RefTarget::Decl { file } = reference.target else {
                continue;
            };
            let Some(open) = self.call_open_paren(reference.range) else {
                continue;
            };
            let Some(formals) = self.member_or_decl_formals(file, None, reference.name) else {
                continue;
            };
            self.omission_rows(
                file,
                reference.name,
                formals,
                open,
                reference.range,
                &mut rows,
            );
        }
        // A member call (`c.m()`): the `formal_annotations` side map keys the
        // DECLARING owner as `<container>.<member>` (collector.rs
        // `formal_owner_key`).
        for edge in &self.report.member_refs {
            if edge.target_container == StringId::NONE {
                continue;
            }
            let Some(open) = self.call_open_paren(edge.range) else {
                continue;
            };
            let Some(formals) = self.member_or_decl_formals(
                edge.target_file,
                Some(edge.target_container),
                edge.target_name,
            ) else {
                continue;
            };
            let owner = self.strings.intern(&format!(
                "{}.{}",
                self.strings.resolve(edge.target_container),
                self.strings.resolve(edge.target_name)
            ));
            self.omission_rows(
                edge.target_file,
                owner,
                formals,
                open,
                edge.range,
                &mut rows,
            );
        }
        // A creation (`C()`), anchored at the type head — the constructor
        // name's own start.
        for (range, name) in &self.type_uses {
            let Some((formals, open, file, owner)) = self.creation_face(*range, *name) else {
                continue;
            };
            let anchor = self.constructor_name_at(*range).unwrap_or(*range);
            let owner = self.strings.intern(&owner);
            self.omission_rows(file, owner, formals, open, anchor, &mut rows);
        }
        rows
    }

    /// One call site's omissions: `_checkForDeprecatedOptional` drops every
    /// formal an argument binds, then reports each remaining
    /// `@Deprecated.optional` formal at the call's callee
    /// (deprecated_functionality_verifier.dart:215-235).
    fn omission_rows(
        &self,
        file: FileId,
        owner: StringId,
        formals: &Formals,
        open: usize,
        anchor: SourceRange,
        rows: &mut Vec<DepRow>,
    ) {
        let Some(elements) = self.index.get(file) else {
            return;
        };
        let (positional, labels) = self.provided_args(open);
        let decl_lib = self.graph.library_of_file(file).unwrap_or(self.lib);
        let omitted = formals.positional.iter().skip(positional).chain(
            formals
                .named
                .iter()
                .filter(|formal| !labels.contains(&formal.name)),
        );
        for formal in omitted {
            let Some(annotations) = elements.formal_annotations(owner, formal.name) else {
                continue;
            };
            let carries = annotations.iter().any(|annotation| {
                crate::annotation::deprecation_kind(
                    annotation,
                    self.strings,
                    self.graph,
                    self.namespaces,
                    decl_lib,
                ) == Some(DeprecationKind::Optional)
            });
            if !carries {
                continue;
            }
            let name = self.strings.resolve(formal.name).to_owned();
            rows.push((
                DEPRECATED_OPTIONAL,
                anchor,
                format!("Omitting an argument for the '{name}' parameter is deprecated."),
                Some(format!("Try passing an argument for '{name}'.")),
            ));
        }
    }

    /// The arguments a call site supplies, at its argument list: the count of
    /// positional arguments and the named labels (each `NamedExpression`
    /// head).
    fn provided_args(&self, open: usize) -> (usize, Vec<StringId>) {
        let tokens = self.tokens.as_slice();
        let Some(close) = self.tokens.matching_end(open) else {
            return (0, Vec::new());
        };
        let mut positional = 0usize;
        let mut labels = Vec::new();
        let mut cursor = open + 1;
        while cursor < close {
            match tokens[cursor].kind {
                TokenKind::Comma => cursor += 1,
                TokenKind::Identifier
                    if tokens
                        .get(cursor + 1)
                        .is_some_and(|t| t.kind == TokenKind::Colon) =>
                {
                    labels.push(self.strings.intern(self.tokens.lexeme(self.source, cursor)));
                    cursor = self.skip_argument(cursor + 2, close);
                }
                _ => {
                    positional += 1;
                    cursor = self.skip_argument(cursor, close);
                }
            }
        }
        (positional, labels)
    }

    /// One argument expression's end: the index of its depth-0 comma (or the
    /// list's closer). Bracket groups are skipped whole, so a nested call's
    /// commas never split the argument.
    fn skip_argument(&self, from: usize, close: usize) -> usize {
        let tokens = self.tokens.as_slice();
        let mut cursor = from;
        while cursor < close {
            match tokens[cursor].kind {
                TokenKind::OpenParen | TokenKind::OpenCurly | TokenKind::OpenSquare => {
                    cursor = self
                        .tokens
                        .matching_end(cursor)
                        .map_or(cursor + 1, |end| end + 1);
                }
                TokenKind::Comma => return cursor,
                _ => cursor += 1,
            }
        }
        cursor
    }

    /// The `ConstructorName` span a type-reference head spells, when the
    /// written tail makes it one: `Foo(…)` (an unnamed creation), `Foo.named(…)`
    /// (an invocation of a named constructor), `Foo.new(…)` (the explicit
    /// unnamed selector), and the `Foo.new` tear-off. A static member access
    /// (`Foo.bar` with no argument list) is none.
    fn constructor_name_at(&self, head: SourceRange) -> Option<SourceRange> {
        let tokens = self.tokens.as_slice();
        let mut cursor = tokens.partition_point(|t| t.utf16_offset < head.end());
        if tokens.get(cursor).is_some_and(|t| t.kind == TokenKind::Lt) {
            cursor = self.angle_group_end(cursor)? + 1;
        }
        match tokens.get(cursor).map(|t| t.kind) {
            Some(TokenKind::OpenParen) => Some(head),
            Some(TokenKind::Period) => {
                let selector = tokens.get(cursor + 1)?;
                if !matches!(selector.kind, TokenKind::Identifier | TokenKind::Keyword(_)) {
                    return None;
                }
                let is_new = self.tokens.lexeme(self.source, cursor + 1) == "new";
                if !is_new && tokens.get(cursor + 2).map(|t| t.kind) != Some(TokenKind::OpenParen) {
                    return None;
                }
                let end = selector.utf16_offset + self.token_utf16_len(cursor + 1);
                Some(SourceRange {
                    offset: head.offset,
                    length: end - head.offset,
                })
            }
            _ => None,
        }
    }

    /// The container row `name` resolves to in this unit's scope, with its
    /// declaring library; `None` for a typedef (no container row) and every
    /// other non-container declaration.
    fn resolved_container_row(&self, name: StringId) -> Option<(&Container, LibraryId)> {
        let file = match self.namespaces.lookup(self.graph, self.lib, name) {
            Lookup::Found(NamespaceEntry::Decl(file)) => file,
            _ => return None,
        };
        let container = self.index.get(file)?.container(name)?;
        Some((
            container,
            self.graph.library_of_file(file).unwrap_or(self.lib),
        ))
    }
}
