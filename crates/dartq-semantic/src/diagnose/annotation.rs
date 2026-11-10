//! The `invalid_annotation_target` face (#2993): an annotation applied
//! where its `@Target({TargetKind.…})` allow-list forbids it (upstream
//! Dart SDK source, pkg/analyzer/lib/src/error/annotation_verifier.dart
//! `AnnotationVerifier._checkKinds`, :373-401 — STATIC_WARNING,
//! diagnostic.g.dart:8306-8314).
//!
//! # The official decision, mirrored
//!
//! `ElementAnnotation.targetKinds`
//! (pkg/analyzer/lib/src/dart/element/annotation_target.dart:94-144)
//! resolves the annotation's element — the unnamed/named constructor of
//! an annotation class, or a top-level getter/variable whose declared
//! type names one — and reads that class's own `@Target(kinds)` constant:
//! the kind names match `TargetKind` by `name`, unknown names drop
//! (`.nonNulls`), and a class with no `@Target` answers `null` (never a
//! report). `@override` is hardcoded to
//! `{field, getter, method, setter}` (:9-14) — the dart:core getter face.
//! `_isValidTarget` (:713-749) then maps the annotated syntax to element
//! kinds (`isValidAnnotationTargetElement`, :31-66: the `type` and
//! `overridableMember` set-membership unions, the `optionalParameter`
//! optional-formal arm, the enum-constant/field split) plus the directive
//! arms: the deprecated `directive` kind admits any directive, and
//! `library` admits the unit's FIRST directive (the unnamed-library
//! era's first-directive carrier).
//!
//! This pass walks the analyzed unit's model rows (containers, members,
//! constructors, formals, top-level declarations) and the token stream
//! (directive annotations — the model carries no directive metadata
//! face), resolves each annotation through the import faces at name
//! level (the [`crate::annotation`] mirror posture), and reports at the
//! annotation's NAME token (`node.name`, the `@` excluded) with the
//! invoked element's display name — `A.new`/`A.named` for a constructor
//! face (the unnamed constructor's element name is `new`, rendered
//! through element.dart:842-847's `displayName` join), the bare spelling
//! for the getter/variable face — and the sorted kind display list
//! joined with the Oxford-comma `or` chain (`commaSeparatedWithOr`,
//! string.dart:64-110).
//!
//! Known under-reports (the name-level posture): annotations on type
//! parameters and local-function declarations (no model row); a const
//! variable face whose initializer is not one bare creation (the
//! `const a = A();` chain rides [`FileElements::const_creation`]); a
//! getter face whose declared type is `Object` (the real meta.dart
//! declares `const Object` — the conformance mock declares the class
//! type, which the oracle reads); and barrel-re-exported annotation
//! classes (the import edge must target the declaring library).

use super::*;
use dartq_resolution::{LocalLookup, Lookup, NamespaceEntry};
use dartq_scanner::{Keyword, TokenKind};

/// One `TargetKind` (meta_meta.dart:43-197) — the bits the official
/// element-kind switch reads. `partOfDirective` and `typeParameter`
/// parse and display but no walked site ever satisfies them (a `part of`
/// directive lives in a part file; type parameters carry no model row).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TKind {
    ClassType,
    Constructor,
    Directive,
    EnumType,
    EnumValue,
    ExportDirective,
    Extension,
    ExtensionType,
    Field,
    Function,
    Library,
    Getter,
    ImportDirective,
    Method,
    MixinType,
    OptionalParameter,
    OverridableMember,
    Parameter,
    PartOfDirective,
    Setter,
    TopLevelVariable,
    Type,
    TypeParameter,
    TypedefType,
}

/// The `TargetKind` enum's `name` → display table (meta_meta.dart's
/// `TargetKind._(displayString, name)` pairs).
const TARGET_KINDS: &[(&str, &str, TKind)] = &[
    ("classType", "classes", TKind::ClassType),
    ("constructor", "constructors", TKind::Constructor),
    ("directive", "directives", TKind::Directive),
    ("enumType", "enums", TKind::EnumType),
    ("enumValue", "enum values", TKind::EnumValue),
    (
        "exportDirective",
        "export directives",
        TKind::ExportDirective,
    ),
    ("extension", "extensions", TKind::Extension),
    ("extensionType", "extension types", TKind::ExtensionType),
    ("field", "fields", TKind::Field),
    ("function", "top-level functions", TKind::Function),
    ("library", "libraries", TKind::Library),
    ("getter", "getters", TKind::Getter),
    (
        "importDirective",
        "import directives",
        TKind::ImportDirective,
    ),
    ("method", "methods", TKind::Method),
    ("mixinType", "mixins", TKind::MixinType),
    (
        "optionalParameter",
        "optional parameters",
        TKind::OptionalParameter,
    ),
    (
        "overridableMember",
        "overridable members",
        TKind::OverridableMember,
    ),
    ("parameter", "parameters", TKind::Parameter),
    (
        "partOfDirective",
        "\"part of\" directives",
        TKind::PartOfDirective,
    ),
    ("setter", "setters", TKind::Setter),
    (
        "topLevelVariable",
        "top-level variables",
        TKind::TopLevelVariable,
    ),
    (
        "type",
        "types (classes, enums, mixins, or typedefs)",
        TKind::Type,
    ),
    ("typeParameter", "type parameters", TKind::TypeParameter),
    ("typedefType", "typedefs", TKind::TypedefType),
];

/// The kind a `@Target` set entry joins when `name` matches (`None` for
/// an unknown name — upstream's `.nonNulls()` drop).
fn kind_of_name(name: &str) -> Option<TKind> {
    TARGET_KINDS
        .iter()
        .find(|(kind_name, ..)| *kind_name == name)
        .map(|(.., kind)| *kind)
}

/// The display string of a kind (the message's sorted-list member).
fn display_of_kind(kind: TKind) -> &'static str {
    TARGET_KINDS
        .iter()
        .find(|(_, .., k)| *k == kind)
        .map(|(_, display, _)| *display)
        .unwrap_or("")
}

/// The sorted display list joined with the official `or` chain
/// (`commaSeparatedWithOr`, string.dart:64-110: one element bare, two
/// joined by ` or `, three-plus Oxford-comma `a, b, or c`).
fn join_with_or(mut displays: Vec<&'static str>) -> String {
    displays.sort_unstable();
    displays.dedup();
    match displays.len() {
        0 => String::new(),
        1 => displays[0].to_string(),
        2 => format!("{} or {}", displays[0], displays[1]),
        _ => {
            let (last, head) = displays.split_last().unwrap();
            format!("{}, or {last}", head.join(", "))
        }
    }
}

/// What an annotation annotates, at the element-kind granularity the
/// official `isValidAnnotationTargetElement` switch reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SiteKind {
    Container(ContainerKind),
    Member {
        kind: MemberKind,
        is_static: bool,
        is_enum_constant: bool,
        container: ContainerKind,
    },
    Constructor,
    FormalParameter {
        optional: bool,
    },
    TopLevelFunction,
    TopLevelGetter,
    TopLevelSetter,
    TopLevelVariable,
    TypeAlias,
    LibraryDirective,
    ImportDirective,
    ExportDirective,
    PartDirective,
}

/// Whether the site is an instance member of an overridable-member
/// container (`_isOverridableMember`, annotation_target.dart:68-92 —
/// classes, extension types, and mixins; enums and extensions never).
fn instance_overridable(site: SiteKind) -> bool {
    match site {
        SiteKind::Member {
            kind,
            is_static,
            container,
            ..
        } => {
            !is_static
                && matches!(
                    container,
                    ContainerKind::Class | ContainerKind::ExtensionType | ContainerKind::Mixin
                )
                && matches!(
                    kind,
                    MemberKind::Field
                        | MemberKind::Getter
                        | MemberKind::Setter
                        | MemberKind::Method
                )
        }
        _ => false,
    }
}

/// `isValidAnnotationTargetElement` (:31-66) + `_isValidTarget`'s
/// directive arms (:713-749) at the site granularity.
fn site_allows(site: SiteKind, kinds: &[TKind]) -> bool {
    let has = |k: TKind| kinds.contains(&k);
    if has(TKind::OverridableMember) && instance_overridable(site) {
        return true;
    }
    match site {
        SiteKind::Container(ContainerKind::Class) => has(TKind::ClassType) || has(TKind::Type),
        SiteKind::Container(ContainerKind::Enum) => has(TKind::EnumType) || has(TKind::Type),
        SiteKind::Container(ContainerKind::Mixin) => has(TKind::MixinType) || has(TKind::Type),
        SiteKind::Container(ContainerKind::Extension) => has(TKind::Extension),
        SiteKind::Container(ContainerKind::ExtensionType) => has(TKind::ExtensionType),
        SiteKind::Member {
            kind: MemberKind::Field,
            is_enum_constant: true,
            ..
        } => has(TKind::EnumValue),
        SiteKind::Member {
            kind: MemberKind::Field,
            ..
        } => has(TKind::Field),
        SiteKind::Member {
            kind: MemberKind::Getter,
            ..
        }
        | SiteKind::TopLevelGetter => has(TKind::Getter),
        SiteKind::Member {
            kind: MemberKind::Setter,
            ..
        }
        | SiteKind::TopLevelSetter => has(TKind::Setter),
        SiteKind::Member {
            kind: MemberKind::Method,
            ..
        } => has(TKind::Method),
        SiteKind::Constructor => has(TKind::Constructor),
        SiteKind::FormalParameter { optional: true } => {
            has(TKind::Parameter) || has(TKind::OptionalParameter)
        }
        SiteKind::FormalParameter { optional: false } => has(TKind::Parameter),
        SiteKind::TopLevelFunction => has(TKind::Function),
        SiteKind::TopLevelVariable => has(TKind::TopLevelVariable),
        SiteKind::TypeAlias => has(TKind::TypedefType) || has(TKind::Type),
        SiteKind::LibraryDirective => has(TKind::Library),
        SiteKind::ImportDirective => has(TKind::ImportDirective),
        SiteKind::ExportDirective => has(TKind::ExportDirective),
        // A `part` directive of a defining unit: only the deprecated
        // `directive` kind and the first-directive `library` arm admit it
        // — both folded into the caller's pre-arms.
        SiteKind::PartDirective => false,
    }
}

impl Engine<'_> {
    /// The whole-unit `invalid_annotation_target` pass: every annotation
    /// the analyzed unit carries, against its `@Target` allow-list.
    pub(crate) fn check_annotation_targets(&mut self) {
        let index = self.index;
        let Some(elements) = index.get(self.file) else {
            return;
        };
        // One row per annotation OCCURRENCE: the model replicates a field
        // group's star onto every member (`@A() int x = 0, y = 0;`), while
        // the oracle reports once per annotation node — the offset set
        // keeps one row per written annotation.
        let mut sites: Vec<(SiteKind, &[crate::model::Annotation])> = Vec::new();
        // Containers, their members, and their annotated constructors.
        for name in elements.container_order() {
            let Some(container) = elements.container(*name) else {
                continue;
            };
            sites.push((SiteKind::Container(container.kind), &container.annotations));
            // The enum-constant boundary: an enum's constants are the
            // static fields declared before the body's first `;` (the
            // header's constant list; an enum with no `;` ends its
            // constants at the closing brace) — a later `static int f` is
            // a real field (`FieldElement.isEnumConstant`).
            let constant_end =
                (container.kind == ContainerKind::Enum).then(|| self.enum_constant_list_end(*name));
            for (_, entry) in container.members.iter() {
                let is_enum_constant = entry.kind == MemberKind::Field
                    && entry.is_static
                    && constant_end.is_some_and(|end| entry.name_range.offset < end);
                sites.push((
                    SiteKind::Member {
                        kind: entry.kind,
                        is_static: entry.is_static,
                        is_enum_constant,
                        container: container.kind,
                    },
                    &entry.annotations,
                ));
            }
            for (outer, ctors) in elements.ctor_annotations.iter() {
                if *outer != *name {
                    continue;
                }
                for (_, annotations) in ctors.iter() {
                    sites.push((SiteKind::Constructor, &annotations[..]));
                }
            }
        }
        // Extension faces (named and unnamed extensions — never extension
        // types, which are container rows).
        for face in elements.extension_faces() {
            sites.push((
                SiteKind::Container(ContainerKind::Extension),
                &face.annotations,
            ));
            for (_, entry) in face.members.iter() {
                sites.push((
                    SiteKind::Member {
                        kind: entry.kind,
                        is_static: entry.is_static,
                        is_enum_constant: false,
                        container: ContainerKind::Extension,
                    },
                    &entry.annotations,
                ));
            }
        }
        // Top-level declarations (functions, getters, setters, variables,
        // typedefs) — the getter/setter/function split reads the `get`/
        // `set` modifier the model keys under one `MicroKind::TopLevelFn`.
        for site in &self.decl_sites {
            let kind = match site.kind {
                DeclKind::TopLevelFn => {
                    if self.token_before_is(site.range.offset, Keyword::Get) {
                        SiteKind::TopLevelGetter
                    } else if self.token_before_is(site.range.offset, Keyword::Set) {
                        SiteKind::TopLevelSetter
                    } else {
                        SiteKind::TopLevelFunction
                    }
                }
                DeclKind::TopLevelVar => SiteKind::TopLevelVariable,
                DeclKind::TypeAlias => SiteKind::TypeAlias,
                DeclKind::Container | DeclKind::Member { .. } | DeclKind::Constructor { .. } => {
                    continue;
                }
            };
            let Some(decl) = elements.decl(site.name) else {
                continue;
            };
            sites.push((kind, &decl.annotations));
        }
        // Formal parameters, through the owner join the annotation
        // recognition already reads.
        for (owner, formals) in elements.formal_annotations.iter() {
            let owner_formals = self.owner_formals(*owner);
            for (formal, annotations) in formals.iter() {
                // `isOptional` (the `optionalParameter` arm,
                // annotation_target.dart:49-51): a formal the owner's
                // table marks non-required — named optionals and `[...]`
                // positionals.
                let optional = owner_formals
                    .as_ref()
                    .and_then(|table| {
                        table
                            .named
                            .iter()
                            .chain(table.positional.iter())
                            .find(|f| f.name == *formal)
                            .map(|f| !f.required)
                    })
                    .unwrap_or(false);
                sites.push((SiteKind::FormalParameter { optional }, &annotations[..]));
            }
        }
        let mut reported: FxHashSet<u32> = FxHashSet::default();
        for (site, annotations) in sites {
            for annotation in annotations {
                self.check_one_annotation_target(site, annotation, &mut reported);
            }
        }
        self.check_directive_annotation_targets(&mut reported);
    }

    /// One annotation occurrence against its allow-list (the per-annotation
    /// `checkAnnotation` → `_checkKinds` seam, :71-78/:373-401).
    fn check_one_annotation_target(
        &mut self,
        site: SiteKind,
        annotation: &crate::model::Annotation,
        reported: &mut FxHashSet<u32>,
    ) {
        if reported.contains(&annotation.range.offset) {
            return;
        }
        let Some((display, kinds, name_len)) = self.resolve_annotation_kinds(annotation) else {
            return;
        };
        if kinds.is_empty() || site_allows(site, &kinds) {
            return;
        }
        reported.insert(annotation.range.offset);
        // The anchor is the annotation's NAME node (`node.name`): the
        // written name without the `@`, and without a trailing
        // constructor selector (`@A.named()` anchors on `A`).
        let anchor = SourceRange {
            offset: annotation.range.offset + 1,
            length: name_len,
        };
        let valid = join_with_or(kinds.iter().map(|k| display_of_kind(*k)).collect());
        self.emit(
            SemanticCode::InvalidAnnotationTarget,
            anchor,
            &display,
            &valid,
        );
    }

    /// The annotation's element face and its `@Target` kinds
    /// (`ElementAnnotation.targetKinds`, annotation_target.dart:94-144):
    /// the display name (the invoked element's name — `A.new`/`A.named`
    /// for constructors, the bare head for the getter/variable face), the
    /// parsed kind set (`None` when no `@Target` rides the class), and
    /// the `node.name` anchor length (the written name without the `@`
    /// and without a trailing constructor selector).
    fn resolve_annotation_kinds(
        &self,
        annotation: &crate::model::Annotation,
    ) -> Option<(String, Vec<TKind>, u32)> {
        let spelling = self.strings.resolve(annotation.name);
        let written_len = annotation.range.length - 1;
        let segments: Vec<&str> = spelling.split('.').collect();
        // The prefix face: a first segment that names an import prefix of
        // the site's library (the scope resolution's own split).
        let head_id_0 = self.strings.intern(segments[0]);
        let (prefix, rest): (Option<&str>, &[&str]) = if segments.len() >= 2
            && self
                .graph
                .import_edges(self.lib)
                .iter()
                .any(|edge| edge.prefix == Some(head_id_0))
        {
            (Some(segments[0]), &segments[1..])
        } else {
            (None, &segments[..])
        };
        // The dart:core `@override` face: hardcoded kinds, no `@Target`
        // read (annotation_target.dart:99-101 with `isOverride`'s
        // core-getter test). The spelled name must not be locally
        // declared, and the namespace hit — when the graph seeds the
        // implicit core import — must be that core getter (a `dart:`
        // library); a rig without the core seed answers `NotFound`, the
        // same unresolved spelling.
        if prefix.is_none() && rest.first() == Some(&"override") && rest.len() == 1 {
            let id = self.strings.intern("override");
            let is_core_override = matches!(self.graph.local(self.lib, id), LocalLookup::Absent)
                && match self.namespaces.lookup(self.graph, self.lib, id) {
                    Lookup::NotFound => true,
                    Lookup::Found(NamespaceEntry::Decl(file)) => self
                        .graph
                        .library_of_file(file)
                        .is_some_and(|lib| self.graph.uri(lib).starts_with("dart:")),
                    _ => false,
                };
            if is_core_override {
                return Some((
                    "override".to_string(),
                    vec![TKind::Field, TKind::Getter, TKind::Method, TKind::Setter],
                    written_len,
                ));
            }
        }
        let head = *rest.first()?;
        let selector = rest.get(1).copied();
        let head_id = self.strings.intern(head);
        let file = self.resolve_head_file(prefix, head_id)?;
        let elements = self.index.get(file)?;
        let decl_lib = self.graph.library_of_file(file)?;
        // The constructor face: a named selector, or an argument group on
        // a bare head (the unnamed `A()` invocation) — both resolve to
        // the head class's own `@Target`.
        if selector.is_some() || annotation.args_text.is_some() {
            let container = elements.container(head_id)?;
            let kinds = self.container_target_kinds(decl_lib, container)?;
            let display = match selector {
                Some(selector) => format!("{head}.{selector}"),
                None => format!("{head}.new"),
            };
            // A named constructor selector is not part of `node.name`
            // (`@A.named()` anchors on `A`, `@p.A.named()` on `p.A` —
            // the selector rides its own node since the
            // constructor-selector grammar).
            let name_len = match selector {
                Some(selector) => written_len - selector.len() as u32 - 1,
                None => written_len,
            };
            return Some((display, kinds, name_len));
        }
        // The getter/variable face: the declared type names the
        // `@Target`-carrying class (a `const a = A();` initializer rides
        // the const-creation capture when no declared type does).
        let decl = elements.decl(head_id)?;
        let ty = match decl.kind {
            MicroKind::TopLevelVar => decl.var_ty,
            _ => decl.return_ty,
        };
        let class_name = match ty {
            Ty::Named { name, .. } => Some(name),
            _ => None,
        }
        .or_else(|| elements.const_creation(head_id).map(|face| face.container))?;
        let class_file = self.resolve_class_file(decl_lib, class_name)?;
        let container = self.index.get(class_file)?.container(class_name)?;
        let kinds = self.container_target_kinds(decl_lib, container)?;
        Some((head.to_string(), kinds, written_len))
    }

    /// The file a (possibly prefixed) annotation head resolves to — the
    /// bare face through the local-then-namespace chain, the prefixed
    /// face through the matching import edge's target library.
    fn resolve_head_file(&self, prefix: Option<&str>, head: StringId) -> Option<FileId> {
        match prefix {
            None => self.resolve_class_file(self.lib, head),
            Some(prefix) => {
                let prefix_id = self.strings.intern(prefix);
                self.graph
                    .import_edges(self.lib)
                    .iter()
                    .find(|edge| {
                        edge.prefix == Some(prefix_id)
                            && edge.target.is_some()
                            && edge.combinators.allows(head)
                    })
                    .and_then(|edge| edge.target)
                    .and_then(|target| match self.graph.local(target, head) {
                        LocalLookup::Found(file) => Some(file),
                        _ => None,
                    })
            }
        }
    }

    /// The file a bare top-level name resolves to within `lib`: the
    /// local declarations first, then the import namespace.
    fn resolve_class_file(&self, lib: LibraryId, name: StringId) -> Option<FileId> {
        match self.graph.local(lib, name) {
            LocalLookup::Found(file) => Some(file),
            _ => match self.namespaces.lookup(self.graph, lib, name) {
                Lookup::Found(NamespaceEntry::Decl(file)) => Some(file),
                _ => None,
            },
        }
    }

    /// The `@Target` kinds of an annotation class — the parsed
    /// `TargetKind.…` entries of its `@Target({...})` argument group
    /// (`None` when the class carries no resolvable `@Target`; an empty
    /// set when it carries one whose entries all drop — the
    /// `kinds.isNotEmpty` silent direction).
    fn container_target_kinds(
        &self,
        decl_lib: LibraryId,
        container: &Container,
    ) -> Option<Vec<TKind>> {
        for annotation in &container.annotations {
            if !self.is_meta_meta_target(annotation, decl_lib) {
                continue;
            }
            let text = annotation.args_text?;
            let text = self.strings.resolve(text);
            let mut kinds = Vec::new();
            let mut rest = text;
            while let Some(index) = rest.find("TargetKind.") {
                rest = &rest[index + "TargetKind.".len()..];
                let name: String = rest
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                    .collect();
                if let Some(kind) = kind_of_name(&name) {
                    kinds.push(kind);
                }
            }
            return Some(kinds);
        }
        None
    }

    /// Whether `annotation` is meta_meta's `Target` class constructor as
    /// written in `lib` — the name-level twin of the angular-meta getter
    /// test (the import edge must target a library declared
    /// `library meta_meta;`, through the declaring library of the
    /// annotation class being walked).
    fn is_meta_meta_target(&self, annotation: &crate::model::Annotation, lib: LibraryId) -> bool {
        let spelling = self.strings.resolve(annotation.name);
        let (prefix, name) = match spelling.rsplit_once('.') {
            Some((prefix, name)) => (Some(prefix), name),
            None => (None, spelling),
        };
        if name != "Target" {
            return false;
        }
        let name_id = self.strings.intern("Target");
        let resolves = |prefix: Option<StringId>| {
            self.graph.import_edges(lib).iter().any(|edge| {
                edge.prefix == prefix
                    && edge
                        .target
                        .is_some_and(|target| self.graph.name(target) == "meta_meta")
                    && edge.combinators.allows(name_id)
            })
        };
        match prefix {
            Some(prefix) => resolves(Some(self.strings.intern(prefix))),
            None => matches!(self.graph.local(lib, name_id), LocalLookup::Absent) && resolves(None),
        }
    }

    /// The formal table an owner join names (a top-level `f`, a member
    /// `C.m`, a constructor `C.<key>` — the unnamed face keyed by the
    /// container's own name), for the formal-optional split.
    fn owner_formals(&self, owner: StringId) -> Option<Box<Formals>> {
        let owner = self.strings.resolve(owner);
        // A top-level executable's join has no dot: its own table.
        let (container, rest) = match owner.split_once('.') {
            Some((container, rest)) => (container, rest),
            None => {
                let id = self.strings.intern(owner);
                let file = self.resolve_head_file(None, id)?;
                return self.index.get(file)?.decl(id)?.formals.clone();
            }
        };
        let container_id = self.strings.intern(container);
        let file = self.resolve_class_file(self.lib, container_id)?;
        let elements = self.index.get(file)?;
        let container_row = elements.container(container_id)?;
        let rest_id = self.strings.intern(rest);
        if let Some(formals) = container_row.ctor_formals.get(&rest_id) {
            return Some(formals.clone());
        }
        if let Some(formals) = container_row.factory_formals.get(&rest_id) {
            return Some(formals.clone());
        }
        container_row
            .members
            .get(&rest_id)
            .and_then(|entry| entry.formals.clone())
    }

    /// The offset just past the enum header's constant list: the enum
    /// body's first `;` (a constant's name token sits before it; a later
    /// `static` field after), or the closing `}` when the body carries no
    /// `;` (an all-constants enum). `u32::MAX` as the not-found fallback
    /// keeps every static field a constant — the under-report direction.
    fn enum_constant_list_end(&self, name: StringId) -> u32 {
        let Some(site) = self
            .decl_sites
            .iter()
            .find(|site| matches!(site.kind, DeclKind::Container) && site.name == name)
        else {
            return u32::MAX;
        };
        let tokens = self.tokens.as_slice();
        let start = tokens.partition_point(|t| t.utf16_offset < site.range.offset);
        for token in &tokens[start..] {
            match token.kind {
                TokenKind::Semicolon | TokenKind::CloseCurly => {
                    return token.utf16_offset + token.byte_len;
                }
                _ => {}
            }
        }
        u32::MAX
    }

    /// Whether the token just before `offset` is `keyword` — the
    /// declaration-site modifier probe (`get`/`set` before the name).
    fn token_before_is(&self, offset: u32, keyword: Keyword) -> bool {
        let tokens = self.tokens.as_slice();
        let index = tokens.partition_point(|t| t.utf16_offset < offset);
        index > 0 && tokens[index - 1].kind == TokenKind::Keyword(keyword)
    }

    /// The directive-annotation faces: annotations bound to the unit's
    /// `import`/`export`/`library`/`part` directives (the model carries
    /// no directive metadata face — the token stream is the source).
    /// Mirrors `_isValidTarget`'s directive arms (:713-749): the
    /// deprecated `directive` kind admits any directive, `library`
    /// admits the unit's FIRST directive, and each directive kind admits
    /// its own kind (the `part of` arm is unreachable in a defining
    /// unit's directive prefix).
    fn check_directive_annotation_targets(&mut self, reported: &mut FxHashSet<u32>) {
        let tokens = self.tokens.as_slice();
        let mut pending: Vec<(String, SourceRange)> = Vec::new();
        let mut first_directive = true;
        let mut index = 0;
        while index < tokens.len() {
            let token = tokens[index];
            match token.kind {
                TokenKind::At => {
                    // The annotation's name: dot-joined segments, then
                    // the argument group is skipped (the binding walks
                    // tokens, not spans).
                    let mut cursor = index + 1;
                    let mut spelling = String::new();
                    let mut name_span: Option<SourceRange> = None;
                    while cursor < tokens.len() {
                        match tokens[cursor].kind {
                            TokenKind::Identifier | TokenKind::Keyword(_) => {
                                let lexeme = self.tokens.lexeme(self.source, cursor);
                                let span = SourceRange {
                                    offset: tokens[cursor].utf16_offset,
                                    length: tokens[cursor].byte_len,
                                };
                                let span =
                                    name_span.map_or(span, |first: SourceRange| SourceRange {
                                        offset: first.offset,
                                        length: span.offset + span.length - first.offset,
                                    });
                                if !spelling.is_empty() {
                                    spelling.push('.');
                                }
                                spelling.push_str(lexeme);
                                name_span = Some(span);
                                cursor += 1;
                                if cursor < tokens.len() && tokens[cursor].kind == TokenKind::Period
                                {
                                    cursor += 1;
                                } else {
                                    break;
                                }
                            }
                            _ => break,
                        }
                    }
                    while matches!(
                        tokens.get(cursor).map(|t| t.kind),
                        Some(TokenKind::OpenParen)
                    ) {
                        cursor = self
                            .tokens
                            .matching_end(cursor)
                            .map_or(cursor + 1, |end| end + 1);
                    }
                    if let Some(span) = name_span {
                        pending.push((spelling, span));
                    }
                    index = cursor;
                }
                TokenKind::Keyword(Keyword::Import)
                | TokenKind::Keyword(Keyword::Export)
                | TokenKind::Keyword(Keyword::Library)
                | TokenKind::Keyword(Keyword::Part) => {
                    let site = match token.kind {
                        TokenKind::Keyword(Keyword::Import) => SiteKind::ImportDirective,
                        TokenKind::Keyword(Keyword::Export) => SiteKind::ExportDirective,
                        TokenKind::Keyword(Keyword::Library) => SiteKind::LibraryDirective,
                        _ => SiteKind::PartDirective,
                    };
                    let is_first = first_directive;
                    first_directive = false;
                    for (spelling, span) in std::mem::take(&mut pending) {
                        let annotation = crate::model::Annotation {
                            name: self.strings.intern(&spelling),
                            range: span,
                            args_text: None,
                        };
                        // The directive pre-arms ride the site check as
                        // extra admitted kinds.
                        self.check_directive_annotation(site, is_first, &annotation, reported);
                    }
                    index += 1;
                }
                _ => {
                    // Any other token ends an annotation group that did
                    // not bind to a directive (a declaration's own
                    // annotations — the model rows already carry them).
                    if !matches!(token.kind, TokenKind::Eof | TokenKind::ScriptTag) {
                        pending.clear();
                    }
                    index += 1;
                }
            }
        }
    }

    /// One directive-bound annotation (the directive pre-arms folded into
    /// the site check: `directive` admits any directive, `library` the
    /// unit's first directive — `_isValidTarget`, :717-729).
    fn check_directive_annotation(
        &mut self,
        site: SiteKind,
        is_first_directive: bool,
        annotation: &crate::model::Annotation,
        reported: &mut FxHashSet<u32>,
    ) {
        if reported.contains(&annotation.range.offset) {
            return;
        }
        // The directive walk's synthesized annotation drops the argument
        // group: an argument-carrying annotation on a directive resolves
        // through the getter face and stays silent (the corpus's
        // directive-bound annotations are all admitted faces) — the
        // under-report direction, never a false row.
        let Some((display, kinds, _)) = self.resolve_annotation_kinds(annotation) else {
            return;
        };
        let pre_armed = !kinds.is_empty()
            && (kinds.contains(&TKind::Directive)
                || (is_first_directive && kinds.contains(&TKind::Library)));
        if kinds.is_empty() || pre_armed || site_allows(site, &kinds) {
            return;
        }
        reported.insert(annotation.range.offset);
        let valid = join_with_or(kinds.iter().map(|k| display_of_kind(*k)).collect());
        self.emit(
            SemanticCode::InvalidAnnotationTarget,
            annotation.range,
            &display,
            &valid,
        );
    }
}
