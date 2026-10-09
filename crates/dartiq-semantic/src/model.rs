//! The M1 element model: what one compilation unit declares, in interned-id
//! form (ticket #119, implementing the four-essential model adjudicated in
//! #131).
//!
//! A [`FileElements`] records exactly four things about its unit:
//!
//! 1. the top-level name set ([`FileElements::decl`]) with a
//!    [`MicroKind`] tag — the adjudicated closed set {class, enum, mixin,
//!    extension, typeAlias, prefix, topLevelFn, topLevelVar, typeParam}
//!    (`prefix` never originates here; it enters a library face from import
//!    edges, see [`crate::library`]);
//! 2. per-container member name sets ([`FileElements::container`]) —
//!    method/getter/setter/field, each with a static/instance bit;
//! 3. direct superclass **name** edges ([`Container::superclass`]) — no
//!    resolved type at M1; `p.S` keeps the bare name `S`;
//! 4. one named-formal-parameter name table per executable
//!    ([`TopDecl::named_formals`], [`MemberEntry::named_formals`]) — the hard
//!    consumer is `undefined_named_parameter`.
//!
//! # Shape discipline
//!
//! * Names are [`StringId`]s; lookups are `FxHashMap` hits on `u32` keys —
//!   no string work, no per-lookup allocation. A model is sized entirely by
//!   its unit's declarations; nothing grows after capture.
//! * Duplicate names keep the **first** entry: a scope's names are unique by
//!   construction, and duplicates are the diagnostic tickets' `duplicate_*`
//!   business, not a modeling event.
//! * Setters are stored under their `name=` spelling, matching the
//!   resolution layer's export-scope convention
//!   (`summary2/library_builder.dart:136-140`). Top-level getters/setters
//!   fold into `topLevelFn` (the adjudicated kind set has no finer tag);
//!   class-level setters are tagged [`MemberKind::Setter`].
//! * Type parameters land in the per-unit name set tagged
//!   [`MicroKind::TypeParam`] (the adjudicated set includes the tag). They
//!   are declaration-scoped in truth; consumers must not treat them as
//!   top-level declarations.

use dartiq_source::StringId;
use rustc_hash::FxHashMap;

/// The micro-kind tag of a name in a library scope (adjudicated closed set).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MicroKind {
    /// A class (including named mixin applications `class C = A with B;`).
    Class,
    /// An enum declaration.
    Enum,
    /// A mixin declaration.
    Mixin,
    /// An extension or extension-type declaration.
    Extension,
    /// A typedef (either style).
    TypeAlias,
    /// An import prefix (`import 'u' as p;`) — faces only, never captured
    /// from a unit.
    Prefix,
    /// A top-level function, getter, or setter (getters/setters under their
    /// lookup spelling: plain `name`, setters as `name=`).
    TopLevelFn,
    /// A top-level variable.
    TopLevelVar,
    /// A type parameter (declaration-scoped in truth; see the module docs).
    TypeParam,
}

/// The kind of a class member.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemberKind {
    /// A method (operator names keep their operator spelling).
    Method,
    /// A getter, under its plain name.
    Getter,
    /// A setter, under its `name=` spelling.
    Setter,
    /// A field (enum constants count as static fields).
    Field,
}

/// The declaration family of a named container.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContainerKind {
    /// A class or named mixin application.
    Class,
    /// An enum declaration.
    Enum,
    /// A mixin declaration.
    Mixin,
    /// An extension declaration (named or unnamed).
    Extension,
    /// An extension-type declaration.
    ExtensionType,
}

/// One class member: kind, static/instance bit, and its named formals.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemberEntry {
    /// The member kind.
    pub kind: MemberKind,
    /// `static` member (enum constants are static).
    pub is_static: bool,
    /// The named formal parameter names (`RequiredNamed`/`OptionalNamed`),
    /// in declaration order. `None` = the member declares none — a definite
    /// "no named formals", not an unknown.
    pub named_formals: Option<Box<[StringId]>>,
}

/// One named container declaration with its member face and superclass edge.
#[derive(Clone, Debug)]
pub struct Container {
    /// The declaration family.
    pub kind: ContainerKind,
    /// The direct superclass name (`extends S` / a named mixin application's
    /// head type), name-level; `None` when absent or unrecoverable.
    pub superclass: Option<StringId>,
    /// The member name set.
    pub members: FxHashMap<StringId, MemberEntry>,
}

/// One top-level declaration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TopDecl {
    /// The micro-kind tag.
    pub kind: MicroKind,
    /// Named formals of a top-level function; `None` otherwise.
    pub named_formals: Option<Box<[StringId]>>,
}

/// Everything one compilation unit contributes to the semantic model.
///
/// Built by [`crate::collector::collect`]; queried per library through
/// [`crate::library`].
#[derive(Clone, Debug, Default)]
pub struct FileElements {
    pub(crate) decls: FxHashMap<StringId, TopDecl>,
    pub(crate) containers: FxHashMap<StringId, Container>,
    pub(crate) extension_members: FxHashMap<StringId, MemberEntry>,
}

impl FileElements {
    /// The top-level declaration under `name`, private names included.
    #[must_use]
    pub fn decl(&self, name: StringId) -> Option<&TopDecl> {
        self.decls.get(&name)
    }

    /// The named container under `name` (unnamed extensions are absent —
    /// their members surface in [`Self::extension_member`]).
    #[must_use]
    pub fn container(&self, name: StringId) -> Option<&Container> {
        self.containers.get(&name)
    }

    /// The extension-instance member pool entry under `name`: the merged
    /// member names of every extension of the unit (named and unnamed,
    /// static bit preserved). Before reporting an instance-scope
    /// `undefined_*`, check this pool — extension application is name-level
    /// opaque at M1.
    #[must_use]
    pub fn extension_member(&self, name: StringId) -> Option<&MemberEntry> {
        self.extension_members.get(&name)
    }

    /// Every top-level declaration with its name, in arbitrary order.
    ///
    /// Consumers feeding the library graph must skip
    /// [`MicroKind::TypeParam`] entries: type parameters are
    /// declaration-scoped, not library faces.
    pub fn declarations(&self) -> impl Iterator<Item = (StringId, &TopDecl)> {
        self.decls.iter().map(|(name, decl)| (*name, decl))
    }

    /// Every named container with its name, in arbitrary order.
    pub fn containers(&self) -> impl Iterator<Item = (StringId, &Container)> {
        self.containers.iter().map(|(name, c)| (*name, c))
    }
}
