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
//! 4. one formal-parameter table per executable
//!    ([`TopDecl::formals`], [`MemberEntry::formals`]) — consumers
//!    `undefined_named_parameter` (named membership) and
//!    `argument_type_not_assignable` (parameter nullability).
//!
//! Batch 2 of the diagnostic tickets (#121) adds the declaration-side facts
//! the wrong-number and static-nullability codes decide on: type-parameter
//! counts, declared [`Ty`]s (fields, variables, formals, return types), and
//! the declaration-arity violation log ([`FileElements::arity_violations`],
//! the [`DuplicateDecl`] pattern: capture-time log, diagnose-time fold).
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

use dartq_source::{SourceRange, StringId};
use rustc_hash::FxHashMap;

/// The M1 static type: the nullability dimension plus a name — the minimal
/// shape the static-nullability codes decide on (types-nullability-spec
/// §2.1–§2.3; the analyzer's `InvalidType` suppresses every question it
/// touches, `subtype.dart:258-260`).
///
/// This is the M1 stand-in for the `dartq-types` interned lattice: no type
/// arguments, no hierarchy, no inference — declared types only, so every
/// decision below is a pure nullability/name comparison. When the M2 type
/// engine wires in, `Ty` folds into `TypeId` and `assignable_from` becomes
/// `TypeSystem::is_assignable`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ty {
    /// No declared type (or a shape M1 does not model): every check
    /// suppresses. Mirrors the oracle's `InvalidType` (rule 0c).
    Unknown,
    /// `Null` — the type of the `null` literal. Semantically nullable
    /// without a suffix (`type.dart:1276`).
    Null,
    /// `dynamic` — top type, semantically nullable (`type.dart:33`).
    Dynamic,
    /// `void` — top type for the return matrix; never an expression type.
    Void,
    /// A named type as written (`int`, `C`, `p.C` keeps `C`, a type
    /// parameter keeps its name), with the syntactic `?` flag. External
    /// names (`int`, `String`) stay here — the nullability rules never
    /// need their members.
    Named {
        /// The declared name as written (display id).
        name: StringId,
        /// The trailing `?` was present.
        nullable: bool,
    },
}

impl Ty {
    /// The every-check-suppresses sentinel.
    pub const UNKNOWN: Ty = Ty::Unknown;

    /// A non-nullable named type.
    #[must_use]
    pub const fn named(name: StringId) -> Ty {
        Ty::Named {
            name,
            nullable: false,
        }
    }

    /// `type_system.dart:1921-` `isStrictlyNonNullable`: false for
    /// `dynamic`, `void`, `Null`, the `?` projection, and unknowns — the
    /// `invalid_null_aware_operator` receiver test.
    #[must_use]
    pub const fn is_strictly_non_nullable(self) -> bool {
        matches!(
            self,
            Ty::Named {
                nullable: false,
                ..
            }
        )
    }

    /// The M1 `isAssignableTo` (`type_system.dart:891-931`): `None` means
    /// the M1 facts cannot decide (oracle would consult the type lattice or
    /// flow analysis) — the caller suppresses, the under-report direction.
    /// `Some(false)` is decidable ONLY from nullability, never from two
    /// distinct named types (a subclass relation could hold).
    #[must_use]
    pub fn assignable_from(self, from: Ty) -> Option<bool> {
        match (from, self) {
            // Unknowns suppress both ways (rule 0c); `void` sources are
            // `use_of_void_result`'s business (unfrozen).
            (Ty::Unknown | Ty::Void, _) | (_, Ty::Unknown) => None,
            // Right Top (`subtype.dart:263-267`): everything assigns into
            // `dynamic`/`void`/`T?` — including `Null` and `void` sources;
            // and `dynamic` assigns into everything (`type_system.dart:930`,
            // non-strict-casts mode).
            (Ty::Dynamic, _) | (_, Ty::Dynamic | Ty::Void | Ty::Named { nullable: true, .. }) => {
                Some(true)
            }
            // Left Null (`subtype.dart:349-369`): `Null <: T1` holds only
            // for `T1 = Null` (Right Top covers that) or the Right Top
            // targets above.
            (Ty::Null, Ty::Null) => Some(true),
            // Only `Null` (and `Never`) assign into `Null`; and Left
            // Nullable (`subtype.dart:389-392`): `S? <: T` needs
            // `Null <: T` — false for a non-nullable named target.
            (Ty::Null, _)
            | (_, Ty::Null)
            | (
                Ty::Named { nullable: true, .. },
                Ty::Named {
                    nullable: false, ..
                },
            ) => Some(false),
            // Same name, both non-nullable: assignable modulo type
            // arguments (invariance ignored — suppression direction).
            (
                Ty::Named { name: a, .. },
                Ty::Named {
                    name: b,
                    nullable: false,
                },
            ) => {
                if a == b {
                    Some(true)
                } else {
                    None
                }
            }
        }
    }
}

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

/// One formal parameter: name, declared type, and the required flag the
/// setter-arity rule reads (`ast_builder.dart:6283`: a setter passes only
/// when the single parameter is required-positional).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Formal {
    /// The parameter name.
    pub name: StringId,
    /// The declared type (`Unknown` when absent).
    pub ty: Ty,
    /// Required-positional (false for optional positional and named).
    pub required: bool,
}

/// The formal-parameter table of one executable: positional parameters in
/// declaration order (required group first), then named parameters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Formals {
    /// Positional parameters, declaration order.
    pub positional: Box<[Formal]>,
    /// Named parameters, declaration order.
    pub named: Box<[Formal]>,
}

impl Formals {
    /// The named formal under `name`, for the named-argument checks.
    #[must_use]
    pub fn named_formal(&self, name: StringId) -> Option<&Formal> {
        self.named.iter().find(|f| f.name == name)
    }
}

/// One class member: kind, static/instance bit, and its executable facts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemberEntry {
    /// The member kind.
    pub kind: MemberKind,
    /// `static` member (enum constants are static).
    pub is_static: bool,
    /// The member's own type parameters (`void m<T>()`); 0 when absent.
    pub type_params: u16,
    /// Declared type of a field (getters/setters/methods: [`Ty::Unknown`]).
    pub var_ty: Ty,
    /// Declared return type of a method/getter/setter (fields:
    /// [`Ty::Unknown`]).
    pub return_ty: Ty,
    /// The formal-parameter table; `None` = the member declares no
    /// parameter list shape worth modeling (fields) — a definite "none",
    /// not an unknown.
    pub formals: Option<Box<Formals>>,
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
    /// The container's own type-parameter count (`class C<T, U>`); 0 when
    /// non-generic. The wrong-number-of-type-arguments decisions read it.
    pub type_params: u16,
}

/// One top-level declaration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TopDecl {
    /// The micro-kind tag.
    pub kind: MicroKind,
    /// The declaration's own type parameters; 0 when non-generic.
    pub type_params: u16,
    /// Declared type of a top-level variable (executables:
    /// [`Ty::Unknown`]).
    pub var_ty: Ty,
    /// Declared return type of a top-level function/getter/setter.
    pub return_ty: Ty,
    /// The formal-parameter table; `None` for non-executables.
    pub formals: Option<Box<Formals>>,
}

impl TopDecl {
    /// A bare name tag with no executable facts (containers, variables
    /// whose type was not captured, type parameters).
    #[must_use]
    pub const fn bare(kind: MicroKind) -> TopDecl {
        TopDecl {
            kind,
            type_params: 0,
            var_ty: Ty::UNKNOWN,
            return_ty: Ty::UNKNOWN,
            formals: None,
        }
    }
}

/// A declaration-arity violation captured at the declaration site: the
/// `wrong_number_of_parameters_for_operator` / `_setter` fold log (the
/// [`DuplicateDecl`] pattern — hash-map order must not reach the report).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArityViolation {
    /// A non-`-` operator with a fixed expected count
    /// (`error_verifier.dart:5688-5727`): `[]=`→2, `[]`/binary/`==`→1,
    /// `~`→0.
    Operator {
        /// The operator spelling as declared.
        name: StringId,
        /// The expected parameter count.
        expected: u16,
        /// The declared parameter count.
        found: u16,
    },
    /// `operator -` with more than one parameter (the minus template takes
    /// only the found count: `... should declare 0 or 1 parameter, but {0}
    /// found.`).
    OperatorMinus {
        /// The declared parameter count.
        found: u16,
    },
    /// A setter whose parameter list is not exactly one required-positional
    /// parameter (`ast_builder.dart:6283-6291`).
    Setter,
}

/// A capture-time arity violation: the code choice plus the name token's
/// span (the oracle reports at the declaration's name).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArityDecl {
    /// The violation.
    pub violation: ArityViolation,
    /// Source span of the declaration's name token.
    pub range: SourceRange,
}

/// A second declaration whose name was already taken in its scope, recorded
/// at capture time (analyzer `duplicate_definition` granularity:
/// `duplicate_definition_verifier.dart` — top-level and class-member scopes
/// keep getters and setters apart, class members compare within their
/// static/instance bit, type parameters compare per list, and a duplicate is
/// diagnosed at the *second* declaration). The model keeps the first entry;
/// this log is the diagnostic surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DuplicateDecl {
    /// The colliding name (lookup spelling: setters under `name=`).
    pub name: StringId,
    /// Source span of the duplicate (second) declaration's name token.
    pub range: SourceRange,
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
    pub(crate) duplicates: Vec<DuplicateDecl>,
    pub(crate) arity_violations: Vec<ArityDecl>,
}

impl FileElements {
    /// The top-level declaration under `name`, private names included.
    #[must_use]
    pub fn decl(&self, name: StringId) -> Option<&TopDecl> {
        self.decls.get(&name)
    }

    /// The named container under `name` (unnamed extensions are absent —
    /// their members surface in [`FileElements::extension_member`]).
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

    /// Every duplicate (second) declaration captured in this unit, in source
    /// order. The keep-first rule of the name maps is unchanged; this log is
    /// what `duplicate_definition` is emitted from.
    #[must_use]
    pub fn duplicates(&self) -> &[DuplicateDecl] {
        &self.duplicates
    }

    /// Every declaration-arity violation captured in this unit, in source
    /// order; folded into the report by [`crate::diagnose`].
    #[must_use]
    pub fn arity_violations(&self) -> &[ArityDecl] {
        &self.arity_violations
    }
}
