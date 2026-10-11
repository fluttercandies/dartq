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

use dartq_source::{SourceRange, StringId, StringTable};
use rustc_hash::FxHashMap;
use std::hash::{Hash, Hasher};

/// The M1 static type: the nullability dimension plus a name — the minimal
/// shape the static-nullability codes decide on (types-nullability-spec
/// §2.1–§2.3; the analyzer's `InvalidType` suppresses every question it
/// touches, `subtype.dart:258-260`).
///
/// This is the M1 stand-in for the `dartq-types` interned lattice: explicit
/// type arguments ride along (the #611 verdict-A slot — a canonical
/// [`ArgListId`] handle so `Ty` stays `Copy`), but there is still no
/// hierarchy, no inference — declared types only. When the M2 type engine
/// wires in, `Ty` folds into `TypeId` and `assignable_from` becomes
/// `TypeSystem::is_assignable`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
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
        /// The explicit type arguments, canonically interned
        /// ([`ArgListId::EMPTY`] when absent). Equal lists share one id,
        /// so derived equality is structural — `List<int> ==
        /// List<int>` across interning orders.
        args: ArgListId,
    },
}

impl Ty {
    /// The every-check-suppresses sentinel.
    pub const UNKNOWN: Ty = Ty::Unknown;

    /// A non-nullable named type without type arguments — the argument-less
    /// shape every pre-#701 construction site keeps ([`ArgPool::named`] is
    /// the with-arguments constructor).
    #[must_use]
    pub const fn named(name: StringId) -> Ty {
        Ty::Named {
            name,
            nullable: false,
            args: ArgListId::EMPTY,
        }
    }

    /// Renders the type in the official display form (`List<int>?`): the
    /// head name, the explicit arguments `, `-joined, then the `?`
    /// projection (the analyzer's `getDisplayName`, type.dart:1425-1467).
    /// [`Ty::Unknown`] renders empty — the diagnoser's message slots for
    /// unknown declared types.
    #[must_use]
    pub fn render(&self, strings: &StringTable, args: &ArgPool) -> String {
        match *self {
            Ty::Unknown => String::new(),
            Ty::Null => "Null".to_string(),
            Ty::Dynamic => "dynamic".to_string(),
            Ty::Void => "void".to_string(),
            Ty::Named {
                name,
                nullable,
                args: list,
            } => {
                let mut text = strings.resolve(name).to_owned();
                let list = args.get(list);
                if !list.is_empty() {
                    text.push('<');
                    for (index, arg) in list.iter().enumerate() {
                        if index > 0 {
                            text.push_str(", ");
                        }
                        text.push_str(&arg.render(strings, args));
                    }
                    text.push('>');
                }
                if nullable {
                    text.push('?');
                }
                text
            }
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
            // non-strict-casts mode — the `strict-casts` overlay rejects the
            // downcast side at the diagnoser, [`crate::diagnose`]'s
            // `Engine::assignable`).
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
            // arguments. The arguments are now *represented*
            // ([`Ty::Named`]'s slot) but still not consulted: invariance
            // stays the suppression direction until the consumption ticket
            // wires the hierarchy in (`List<int>` vs `List<num>` is `None`,
            // never a decidable `false`).
            (
                Ty::Named { name: a, .. },
                Ty::Named {
                    name: b,
                    nullable: false,
                    ..
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

/// Identity of an interned type-argument list inside an [`ArgPool`] —
/// [`Ty::Named`]'s argument slot (#611 verdict A: a pool handle keeps `Ty`
/// `Copy`, and canonical interning decides equality, so the slot never
/// degrades into position-comparing `PoolRange`-style fake equality).
///
/// Slot `0` is the reserved empty list: argument-less named types
/// ([`Ty::named`]) carry it without touching any pool, so the empty face is
/// one constant and the pre-slot constructors keep their exact identity.
/// `Eq`/`Hash` on this newtype *is* structural list equality — the pool
/// only hands out ids through canonical interning, the same contract as
/// `dartq_types::TypeId`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ArgListId(u32);

impl ArgListId {
    /// The reserved empty argument list.
    pub const EMPTY: ArgListId = ArgListId(0);
}

/// The canonical interner of type-argument lists backing
/// [`Ty::Named`]'s slot. Equal lists share one [`ArgListId`] regardless of
/// interning order or pool history, so derived equality on `Ty` is
/// structural (`List<int> == List<int>`) without any string or deep-list
/// comparison on the query path — the O(1) id comparison the interned
/// lattice uses.
///
/// One pool belongs to one model universe ([`FileElements::type_args`]):
/// ids are canonical *within their pool*, so [`Ty`] values compare equal
/// only inside the universe that interned their arguments. Arguments are
/// captured bottom-up (inner lists first), so an element's own slot is
/// already canonical when its list interns — no recursion at intern or
/// dedup time.
#[derive(Clone, Debug)]
pub struct ArgPool {
    /// Interned lists; slot `0` is the reserved empty list.
    lists: Vec<Box<[Ty]>>,
    /// Structural hash → every slot with that hash. A hit is confirmed by
    /// slice equality before reuse (64-bit hashes collide rarely; the
    /// bucket keeps the answer exact rather than probabilistic).
    at: FxHashMap<u64, Vec<u32>>,
}

impl Default for ArgPool {
    fn default() -> Self {
        Self {
            lists: vec![Box::new([])],
            at: FxHashMap::default(),
        }
    }
}

impl ArgPool {
    /// Interns `args` canonically and returns the named type
    /// `name<...>` (with its `?` when `nullable`) — the with-arguments
    /// constructor; [`Ty::named`] is its argument-less constant shortcut
    /// and produces the identical value for an empty `args`.
    pub fn named(&mut self, name: StringId, nullable: bool, args: &[Ty]) -> Ty {
        let args = self.intern(args);
        Ty::Named {
            name,
            nullable,
            args,
        }
    }

    /// Interns one list and returns its canonical slot. The empty list is
    /// the reserved slot `0` and never interns; a dedup hit reuses the
    /// stored slot without appending.
    fn intern(&mut self, args: &[Ty]) -> ArgListId {
        if args.is_empty() {
            return ArgListId::EMPTY;
        }
        let hash = hash_args(args);
        if let Some(slots) = self.at.get(&hash) {
            for &slot in slots {
                if &*self.lists[slot as usize] == args {
                    return ArgListId(slot);
                }
            }
        }
        let slot = u32::try_from(self.lists.len()).expect("argument pool exceeds u32::MAX");
        self.lists.push(Box::from(args));
        self.at.entry(hash).or_default().push(slot);
        ArgListId(slot)
    }

    /// The interned list at `id`.
    ///
    /// # Panics
    /// Panics when `id` did not come from this pool (an out-of-range slot
    /// is a program bug, not a recoverable condition).
    #[must_use]
    pub fn get(&self, id: ArgListId) -> &[Ty] {
        &self.lists[id.0 as usize]
    }

    /// Number of interned non-empty argument lists.
    #[must_use]
    pub fn len(&self) -> usize {
        self.lists.len() - 1
    }

    /// Whether no non-empty list has been interned.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lists.len() == 1
    }
}

/// Folds every element's canonical identity — name, nullability, and its
/// own (already interned) argument slot — into one 64-bit hash. Children
/// are canonical, so their slot hashes their whole structure; no recursion
/// here.
fn hash_args(args: &[Ty]) -> u64 {
    let mut hasher = rustc_hash::FxHasher::default();
    for arg in args {
        arg.hash(&mut hasher);
    }
    hasher.finish()
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

/// One representation-field capture of an extension type's primary
/// constructor (`extension type E(int x)` — the `int x` formal). The
/// semantic representation checks (`error_verifier.dart:5340-5460`
/// `_checkForExtensionTypeRepresentationErrorCodes` and the self-reference
/// check at :5343-5354) read the name, its span, and the declared type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepFormal {
    /// The formal's facts (name, declared type, required bit).
    pub formal: Formal,
    /// Source span of the formal's name token (the emission anchor).
    pub name_range: SourceRange,
}

/// One extension declaration's application face: the identity the implicit
/// member application resolves against (`extension_member_resolver.dart:85-128`
/// `findExtension` + `applicable_extensions.dart:157-171` `applicableTo`).
/// Named and unnamed extensions both produce one; extension types do not
/// (their members surface on the extension type's own container face).
#[derive(Clone, Debug)]
pub struct ExtensionFace {
    /// The declaring extension's name; `None` for an unnamed extension (the
    /// ambiguity message renders the `on` type instead,
    /// `extension_member_resolver.dart:119-124`).
    pub name: Option<StringId>,
    /// The `on` clause type name as written (a qualified `on p.T` keeps the
    /// bare `T`, the model's `p.S` convention). `None` when the clause never
    /// produced an honest name (missing clause recovery, synthetic
    /// reference) — the face can never apply and its members are suppressed
    /// from application (the oracle's `extendedType` fails to resolve; an
    /// inapplicable extension never gates an `undefined_*`).
    pub on_type: Option<StringId>,
    /// The extension's members under their lookup spellings (setters under
    /// `name=`), static bit preserved.
    pub members: FxHashMap<StringId, MemberEntry>,
}

/// One class member: kind, static/instance bit, and its executable facts.
/// The modifier bits are the declaration's own flat flags — orthogonal in
/// the grammar, so they stay flat here.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemberEntry {
    /// The member kind.
    pub kind: MemberKind,
    /// `static` member (enum constants are static).
    pub is_static: bool,
    /// The `abstract` modifier was present (abstract fields).
    pub is_abstract: bool,
    /// The `external` modifier was present (external members).
    pub is_external: bool,
    /// The field group's `final` modifier (`final int x;`); false for every
    /// other member shape. The abstract-completeness check splits an
    /// abstract field's missing halves with it (a final field has no
    /// setter).
    pub is_final: bool,
    /// The member's own type parameters (`void m<T>()`); 0 when absent.
    pub type_params: u16,
    /// The member's own type-parameter names, in declaration order (the
    /// `wrong_number_of_type_arguments_function` display renders them).
    pub type_param_names: Box<[StringId]>,
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
    /// The `abstract` modifier was present on a class-family header
    /// (classes and named mixin applications; always false for the other
    /// kinds, which cannot be abstract). The abstract-completeness check
    /// reads it (concrete classes only).
    pub is_abstract: bool,
    /// The `base` keyword was present on the header (classes, mixins, mixin
    /// classes). The class-modifier restriction checks read it
    /// (`dartq_resolution::modifiers`).
    pub is_base: bool,
    /// The `final` keyword was present on a class-family header.
    pub is_final: bool,
    /// The `interface` keyword was present on a class-family header.
    pub is_interface: bool,
    /// The declaration is the `mixin class` form.
    pub is_mixin_class: bool,
    /// The direct superclass name (`extends S` / a named mixin application's
    /// head type), name-level; `None` when absent or unrecoverable.
    pub superclass: Option<StringId>,
    /// The member name set.
    pub members: FxHashMap<StringId, MemberEntry>,
    /// The container's own type-parameter count (`class C<T, U>`); 0 when
    /// non-generic. The wrong-number-of-type-arguments decisions read it.
    pub type_params: u16,
    /// The `sealed` modifier was present (the exhaustiveness splitter,
    /// `types/sealed.dart`).
    pub sealed: bool,
    /// The `with` mixin-application names as written (class/enum application
    /// order; a named mixin application's `with` list included).
    pub with_clause: Vec<StringId>,
    /// The `implements` interface names as written (interface edges).
    pub implements_clause: Vec<StringId>,
    /// The `on` constraint names of a mixin, as written (constraint checks,
    /// not supertype edges); empty for every other container kind.
    pub on_clause: Vec<StringId>,
    /// Header-window type names no clause event claimed: the `extends`
    /// overflow types the oracle throws away (`ast_builder.dart:3972-3982`
    /// `handleClassExtends` "throw them away for now and pick the first
    /// one") and recovery residue. Never consulted as super-faces; hashed
    /// into the API signature so an L2-relevant edit cannot hide here.
    pub header_residue: Vec<StringId>,
    /// An extension type's primary-constructor formals, in declaration
    /// order (the representation-field family's facts); empty elsewhere.
    pub primary_formals: Box<[RepFormal]>,
    /// An enum's constants in declaration order (the exhaustiveness
    /// witness order follows it, `types/enum.dart`); empty elsewhere.
    pub enum_constants: Vec<StringId>,
    /// The named constructor selectors the container declares (`A.named`
    /// keeps `named`). Unnamed constructors and the implicit default are
    /// [`Container::has_unnamed_constructor`]'s business.
    pub constructors: Box<[StringId]>,
    /// The container can be constructed without a selector: no explicit
    /// constructor exists (the implicit default), or an unnamed one was
    /// declared. A named-only declaration (`A.named();`) drops this to
    /// `false` — the `undefined_constructor_in_initializer` /
    /// `undefined_enum_constructor` unnamed-selector queries read it.
    pub has_unnamed_constructor: bool,
    /// Constructor formals by lookup spelling: the selector for a named
    /// constructor (`A.named` → `named`), the container's own name for the
    /// unnamed face (`A()` → `A` — the same spelling a creation's folded
    /// head row carries). The `unused_element_parameter` constructor face
    /// reads it (`unused_local_elements_verifier.dart:780-792` walks every
    /// formal list, constructors included).
    pub ctor_formals: FxHashMap<StringId, Box<Formals>>,
    /// The declaration's metadata, in written order (name-level,
    /// [`Annotation`]). The meta-getter recognition (`crate::annotation`)
    /// reads it; members' own metadata is not part of the container face.
    pub annotations: Box<[Annotation]>,
}

/// One declaration annotation, name-level: the annotation's spelled name
/// and its own span. Argument lists are not captured — the meta-getter
/// faces the recognition helper reads ([`crate::annotation`]) need
/// presence and name only.
#[derive(Clone, Debug)]
pub struct Annotation {
    /// The name as written: the head identifier for a bare `@sealed`, the
    /// dot-joined spelling for a qualified `@meta.sealed`.
    pub name: StringId,
    /// The annotation's own span, `@` through the name's last segment
    /// (argument groups are outside it; the official annotation-anchored
    /// reports anchor the `@` sign).
    pub range: SourceRange,
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
    pub(crate) container_order: Vec<StringId>,
    pub(crate) extension_faces: Vec<ExtensionFace>,
    pub(crate) duplicates: Vec<DuplicateDecl>,
    pub(crate) arity_violations: Vec<ArityDecl>,
    /// The unit's canonical argument pool: every [`Ty::Named`] slot of
    /// this model indexes into it. One pool per model universe — ids from
    /// another unit's pool are meaningless here.
    pub(crate) type_args: ArgPool,
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

    /// The container names in declaration order (`containers` itself is a
    /// hash map; the exhaustiveness face needs the source order for the
    /// sealed-split witness order).
    #[must_use]
    pub fn container_order(&self) -> &[StringId] {
        &self.container_order
    }

    /// Every extension application face of the unit, in declaration order
    /// (named and unnamed extensions; extension types never produce one).
    /// The implicit-application checks filter by `on` type and report the
    /// ambiguity over the applicable candidates
    /// ([`ExtensionFace`]).
    #[must_use]
    pub fn extension_faces(&self) -> &[ExtensionFace] {
        &self.extension_faces
    }

    /// Whether any extension face of the unit declares `name` under its
    /// lookup spelling — the conservative gate for expression-typing paths
    /// where the receiver type is unknown (the resolve-side analog of the
    /// old merged pool: any-declaration suppresses, never false-positives).
    #[must_use]
    pub fn has_extension_member(&self, name: StringId) -> bool {
        self.extension_faces
            .iter()
            .any(|face| face.members.contains_key(&name))
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

    /// The unit's canonical argument pool ([`Ty::Named`] slots index into
    /// it); capture feeds it through [`ArgPool::named`].
    #[must_use]
    pub fn type_args(&self) -> &ArgPool {
        &self.type_args
    }

    /// The pool, mutable — the capture side's intern entry.
    pub fn type_args_mut(&mut self) -> &mut ArgPool {
        &mut self.type_args
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dartq_source::StringTable;
    use std::hash::{DefaultHasher, Hash, Hasher};

    /// `int`, `String`, `List`, `Map` over one shared table/pool pair.
    struct Fix {
        strings: StringTable,
        pool: ArgPool,
        int: StringId,
        string: StringId,
        list: StringId,
        map: StringId,
    }

    impl Fix {
        fn new() -> Self {
            let strings = StringTable::new();
            let intern = |text: &str| strings.intern(text);
            Self {
                int: intern("int"),
                string: intern("String"),
                list: intern("List"),
                map: intern("Map"),
                strings,
                pool: ArgPool::default(),
            }
        }

        fn render(&self, ty: Ty) -> String {
            ty.render(&self.strings, &self.pool)
        }
    }

    /// Argument-less construction never touches the pool, and the
    /// with-arguments constructor returns the identical value for an empty
    /// list — the pre-slot construction face is preserved bit for bit.
    #[test]
    fn argument_less_types_never_touch_the_pool() {
        let fix = Fix::new();
        let mut pool = ArgPool::default();
        let plain = Ty::named(fix.int);
        let via_pool = pool.named(fix.int, false, &[]);
        assert_eq!(plain, via_pool);
        assert_eq!(via_pool, pool.named(fix.int, false, &[]));
        assert!(pool.is_empty(), "the empty list must never intern");
    }

    /// Equal argument lists share one slot regardless of interning order —
    /// derived `PartialEq`/`Hash` on `Ty` is structural equality
    /// (`List<int> == List<int>`), the canonical-intern contract.
    #[test]
    fn equal_arguments_intern_to_one_slot() {
        let fix = Fix::new();
        let mut pool = ArgPool::default();
        let int = Ty::named(fix.int);
        let first = pool.named(fix.list, false, &[int]);
        let second = pool.named(fix.list, false, &[int]);
        assert_eq!(first, second);
        assert_eq!(pool.len(), 1, "the second intern must dedup");

        let (mut left, mut right) = (DefaultHasher::new(), DefaultHasher::new());
        first.hash(&mut left);
        second.hash(&mut right);
        assert_eq!(left.finish(), right.finish());
    }

    /// Different arguments, argument count, or nullability are different
    /// types: the slot compares, never collapses.
    #[test]
    fn different_arguments_are_different_types() {
        let fix = Fix::new();
        let mut pool = ArgPool::default();
        let (int, string) = (Ty::named(fix.int), Ty::named(fix.string));
        let list_int = pool.named(fix.list, false, &[int]);
        assert_ne!(list_int, pool.named(fix.list, false, &[string]));
        assert_ne!(
            list_int,
            pool.named(fix.list, false, &[int, string]),
            "arity is part of the identity"
        );
        assert_ne!(
            list_int,
            pool.named(fix.list, true, &[int]),
            "the ? projection is part of the identity"
        );
    }

    /// Nested arguments (`Map<String, List<int>>`) dedup through the
    /// inner slots: re-interning the same nest shares every slot, and a
    /// one-bit inner difference (`List<int?>`) forks the outer slot.
    #[test]
    fn nested_arguments_are_canonical() {
        let fix = Fix::new();
        let mut pool = ArgPool::default();
        let (int, string) = (Ty::named(fix.int), Ty::named(fix.string));

        let list_int = pool.named(fix.list, false, &[int]);
        let first = pool.named(fix.map, false, &[string, list_int]);

        let list_int_again = pool.named(fix.list, false, &[int]);
        let second = pool.named(fix.map, false, &[string, list_int_again]);
        assert_eq!(first, second);

        let list_intq = pool.named(fix.list, true, &[int]);
        assert_ne!(first, pool.named(fix.map, false, &[string, list_intq]));
        assert_eq!(
            pool.len(),
            3,
            "slots: [int], [String, List<int>] (the twin deduped), [String, List<int>?]"
        );
    }

    /// The display face renders the official `Name<Arg, ...>?` form. Every
    /// type interns through the harness's own pool — the same pool the
    /// render reads — per the one-pool-one-universe contract.
    #[test]
    fn display_renders_the_argument_face() {
        let mut fix = Fix::new();
        let (int, string) = (Ty::named(fix.int), Ty::named(fix.string));
        let list_int = fix.pool.named(fix.list, false, &[int]);
        let map = fix.pool.named(fix.map, false, &[string, list_int]);

        assert_eq!(fix.render(Ty::named(fix.int)), "int");
        assert_eq!(fix.render(list_int), "List<int>");
        assert_eq!(fix.render(map), "Map<String, List<int>>");
        let nullable_list = fix.pool.named(fix.list, true, &[int]);
        assert_eq!(fix.render(nullable_list), "List<int>?");
        assert_eq!(fix.render(Ty::Dynamic), "dynamic");
        assert_eq!(fix.render(Ty::Unknown), "");
    }
}
